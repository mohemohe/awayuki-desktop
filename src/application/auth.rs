use crate::api::client::ApiClient;
use crate::api::detect::detect_server_kind;
use crate::api::kind::ServerKind;
use crate::application::desktop::{persist_login_session, AppSnapshot, RuntimeState};
use crate::auth::callback_server;
use crate::auth::session::AccountSession;
use crate::bluesky::auth::login_with_app_password;
use crate::bluesky::client::DEFAULT_BLUESKY_HOST;
use crate::ipc::dto::{CancelLoginFlowRequest, LoginBlueskyRequest, LoginInstanceRequest};
use crate::ipc::error::{AppError, AppErrorCode};
use crate::mastodon::client::MastodonClient;
use crate::mastodon::oauth::OAuthFlow;
use crate::mastodon::types::account::Account;
use crate::misskey::auth::MiAuthFlow;
use crate::misskey::client::MisskeyClient;
use crate::observability::OperationContext;

/// Preserve the login use-case boundary: remote authentication and account
/// verification must finish before the SQLite transaction starts. A failed
/// provider call therefore cannot create a partial portable account row.
async fn authenticate_then_persist<
    T,
    U,
    E,
    Authenticate,
    AuthenticateFuture,
    Persist,
    PersistFuture,
>(
    authenticate: Authenticate,
    persist: Persist,
) -> Result<U, E>
where
    Authenticate: FnOnce() -> AuthenticateFuture,
    AuthenticateFuture: std::future::Future<Output = Result<T, E>>,
    Persist: FnOnce(T) -> PersistFuture,
    PersistFuture: std::future::Future<Output = Result<U, E>>,
{
    let authenticated = authenticate().await?;
    persist(authenticated).await
}

pub(crate) async fn login_with_instance_domain(
    state: &RuntimeState,
    request: LoginInstanceRequest,
) -> Result<AppSnapshot, AppError> {
    let mut operation = OperationContext::start(
        "login_with_instance_domain",
        request.operation_id.as_deref(),
        None,
    );
    let domain = normalize_login_domain(&request.domain)
        .map_err(|error| operation.finish_error_code(AppErrorCode::Validation, error))?;
    state.login_flow_manager().cancel_all();
    let Some(login) = state.login_flow_manager().begin(operation.id()) else {
        return Err(operation.finish_error_code(
            AppErrorCode::Validation,
            "login operation is already active",
        ));
    };
    operation.phase("api");
    let result = tokio::select! {
        _ = login.token().cancelled() => {
            return Err(operation.finish_error_code(AppErrorCode::Cancelled, "login flow cancelled"));
        }
        result = authenticate_then_persist(
            || run_login_flow(&domain),
            |(session, kind)| persist_login_session(state, session, kind),
        ) => result,
    };
    match result {
        Ok(snapshot) => {
            operation.finish_ok();
            Ok(snapshot)
        }
        Err(error) => Err(operation.finish_error(error)),
    }
}

pub(crate) async fn login_with_bluesky_app_password(
    state: &RuntimeState,
    request: LoginBlueskyRequest,
) -> Result<AppSnapshot, AppError> {
    let mut operation = OperationContext::start(
        "login_with_bluesky_app_password",
        request.operation_id.as_deref(),
        None,
    );
    let identifier = request.identifier.trim().to_string();
    if identifier.is_empty() || request.password.is_empty() {
        return Err(operation.finish_error_code(
            AppErrorCode::Validation,
            "Bluesky identifier and app password are required",
        ));
    }
    state.login_flow_manager().cancel_all();
    let Some(login) = state.login_flow_manager().begin(operation.id()) else {
        return Err(operation.finish_error_code(
            AppErrorCode::Validation,
            "login operation is already active",
        ));
    };
    operation.phase("api");
    let result = tokio::select! {
        _ = login.token().cancelled() => {
            return Err(operation.finish_error_code(AppErrorCode::Cancelled, "login flow cancelled"));
        }
        result = authenticate_then_persist(
            || run_bluesky_login(&identifier, &request.password),
            |(session, kind)| persist_login_session(state, session, kind),
        ) => result,
    };
    match result {
        Ok(snapshot) => {
            operation.finish_ok();
            Ok(snapshot)
        }
        Err(error) => Err(operation.finish_error(error)),
    }
}

pub(crate) async fn cancel_login_flow(
    state: &RuntimeState,
    request: CancelLoginFlowRequest,
) -> Result<bool, AppError> {
    let mut operation =
        OperationContext::start("cancel_login_flow", request.operation_id.as_deref(), None);
    if uuid::Uuid::parse_str(&request.target_operation_id).is_err() {
        return Err(operation.finish_error_code(
            AppErrorCode::Validation,
            "target operation ID must be a UUID",
        ));
    }
    let cancelled = state
        .login_flow_manager()
        .cancel(&request.target_operation_id);
    operation.finish_ok();
    Ok(cancelled)
}

fn normalize_login_domain(input: &str) -> Result<String, String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err("Please enter an instance domain".to_string());
    }
    let normalized_url = trimmed
        .get(..8)
        .filter(|prefix| prefix.eq_ignore_ascii_case("https://"))
        .map(|_| format!("https://{}", &trimmed[8..]))
        .or_else(|| {
            trimmed
                .get(..7)
                .filter(|prefix| prefix.eq_ignore_ascii_case("http://"))
                .map(|_| format!("http://{}", &trimmed[7..]))
        });
    let domain = if let Some(url) = normalized_url {
        let parsed = url::Url::parse(&url).map_err(|_| "Invalid instance domain".to_string())?;
        if !parsed.username().is_empty() || parsed.password().is_some() {
            return Err("Instance domain must not contain credentials".to_string());
        }
        parsed
            .host_str()
            .unwrap_or_default()
            .trim_end_matches('.')
            .to_string()
    } else {
        trimmed
            .split('/')
            .next()
            .unwrap_or_default()
            .trim()
            .trim_end_matches('.')
            .to_string()
    };
    if domain.is_empty() {
        return Err("Please enter an instance domain".to_string());
    }
    Ok(domain.to_lowercase())
}

async fn run_login_flow(domain: &str) -> Result<(AccountSession, ServerKind), String> {
    let kind = detect_server_kind(domain)
        .await
        .map_err(|error| error.to_string())?;
    tracing::info!(domain, ?kind, "Detected server kind for login");
    match kind {
        ServerKind::Mastodon | ServerKind::Paon => run_mastodon_oauth(domain, kind).await,
        ServerKind::Misskey => run_misskey_miauth(domain, kind).await,
        ServerKind::Bluesky => Err(
            "Bluesky cannot be configured via instance domain; use the Bluesky login form below."
                .to_string(),
        ),
    }
}

async fn run_mastodon_oauth(
    domain: &str,
    kind: ServerKind,
) -> Result<(AccountSession, ServerKind), String> {
    let callback_listener = callback_server::CallbackListener::bind()
        .await
        .map_err(|error| error.to_string())?;
    let mut flow =
        OAuthFlow::new(domain, callback_listener.port()).map_err(|error| error.to_string())?;
    flow.prepare().await.map_err(|error| error.to_string())?;
    let auth_url = flow
        .authorize_url()
        .ok_or_else(|| "Failed to generate authorization URL".to_string())?;
    let expected_state = flow.state().to_string();

    tracing::info!("Opening browser for Mastodon authorization");
    open::that(&auth_url).map_err(|error| error.to_string())?;
    let (_, code) = callback_listener
        .wait_for_callback(&[("state", expected_state.as_str())], &["code"])
        .await
        .map_err(|error| error.to_string())?;
    let token_response = flow
        .exchange_code(&code)
        .await
        .map_err(|error| error.to_string())?;
    let instance = flow
        .instance
        .as_ref()
        .ok_or_else(|| "No instance info".to_string())?;
    let streaming_url = instance
        .streaming_url()
        .unwrap_or(&format!("wss://{domain}"))
        .to_string();
    let client = ApiClient::mastodon_with_kind(
        MastodonClient::new(domain, token_response.access_token, streaming_url)
            .map_err(|error| error.to_string())?,
        kind,
    );
    session_from_verified_client(client, domain, kind).await
}

async fn run_misskey_miauth(
    domain: &str,
    kind: ServerKind,
) -> Result<(AccountSession, ServerKind), String> {
    let callback_listener = callback_server::CallbackListener::bind()
        .await
        .map_err(|error| error.to_string())?;
    let flow =
        MiAuthFlow::new(domain, callback_listener.port()).map_err(|error| error.to_string())?;
    let expected_session = flow.session_id().to_string();
    tracing::info!("Opening browser for Misskey authorization");
    open::that(flow.authorize_url()).map_err(|error| error.to_string())?;
    callback_listener
        .wait_for_callback(&[("session", expected_session.as_str())], &["session"])
        .await
        .map_err(|error| error.to_string())?;
    let result = flow.check().await.map_err(|error| error.to_string())?;
    let client = ApiClient::misskey(
        MisskeyClient::new(domain, result.token, format!("wss://{domain}"))
            .map_err(|error| error.to_string())?,
    );
    session_from_verified_client(client, domain, kind).await
}

async fn run_bluesky_login(
    identifier: &str,
    password: &str,
) -> Result<(AccountSession, ServerKind), String> {
    let domain = DEFAULT_BLUESKY_HOST;
    let client = ApiClient::bluesky(
        login_with_app_password(domain, identifier, password, format!("wss://{domain}"))
            .await
            .map_err(|error| error.to_string())?,
    );
    session_from_verified_client(client, domain, ServerKind::Bluesky).await
}

async fn session_from_verified_client(
    client: ApiClient,
    domain: &str,
    kind: ServerKind,
) -> Result<(AccountSession, ServerKind), String> {
    let account = client
        .verify_credentials()
        .await
        .map_err(|error| error.to_string())?;
    let acct = normalized_account_key(&account, domain);
    tracing::info!(acct, ?kind, "Login verified");
    Ok((
        AccountSession {
            acct,
            domain: domain.to_string(),
            client,
            account_info: account,
        },
        kind,
    ))
}

fn normalized_account_key(account: &Account, domain: &str) -> String {
    if account.acct.contains('@') {
        account.acct.clone()
    } else {
        format!("{}@{domain}", account.acct)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[test]
    fn normalizes_login_domain_without_accepting_an_empty_host() {
        assert_eq!(
            normalize_login_domain(" HTTPS://Example.Social/path ").unwrap(),
            "example.social"
        );
        assert!(normalize_login_domain(" https:// ").is_err());
    }

    #[tokio::test]
    async fn provider_authentication_finishes_before_portable_account_transaction() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let authenticate_events = events.clone();
        let persist_events = events.clone();

        let result = authenticate_then_persist(
            move || async move {
                authenticate_events
                    .lock()
                    .expect("event lock")
                    .push("api-complete");
                Ok::<_, String>("verified-session")
            },
            move |session| async move {
                assert_eq!(session, "verified-session");
                persist_events
                    .lock()
                    .expect("event lock")
                    .push("sqlite-transaction");
                Ok::<_, String>(())
            },
        )
        .await;

        assert!(result.is_ok());
        assert_eq!(
            events.lock().expect("event lock").as_slice(),
            ["api-complete", "sqlite-transaction"]
        );
    }

    #[tokio::test]
    async fn provider_failure_never_starts_portable_account_transaction() {
        let persisted = Arc::new(Mutex::new(false));
        let persisted_for_callback = persisted.clone();

        let result = authenticate_then_persist(
            || async { Err::<(), _>("provider rejected login") },
            move |_| async move {
                *persisted_for_callback.lock().expect("persisted lock") = true;
                Ok::<_, &str>(())
            },
        )
        .await;

        assert_eq!(result, Err("provider rejected login"));
        assert!(!*persisted.lock().expect("persisted lock"));
    }
}
