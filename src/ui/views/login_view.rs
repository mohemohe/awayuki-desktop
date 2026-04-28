use gpui::prelude::*;
use gpui::{div, px, rgb, AsyncApp, Context, Entity, EventEmitter, SharedString, WeakEntity, Window};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::input::{Input, InputState};
use gpui_tokio_bridge::Tokio;
use sqlx::SqlitePool;

use crate::api::client::ApiClient;
use crate::api::detect::detect_server_kind;
use crate::api::kind::ServerKind;
use crate::auth::callback_server;
use crate::auth::credential_store::CredentialStore;
use crate::auth::session::AccountSession;
use crate::bluesky::auth::login_with_app_password;
use crate::bluesky::client::DEFAULT_BLUESKY_HOST;
use crate::mastodon::client::MastodonClient;
use crate::mastodon::oauth::OAuthFlow;
use crate::misskey::auth::MiAuthFlow;
use crate::misskey::client::MisskeyClient;
use crate::state::app_state::AppState;

pub enum LoginEvent {
    LoggedIn(AccountSession, ServerKind),
    Cancelled,
}

pub struct LoginView {
    domain_input: Entity<InputState>,
    bsky_id_input: Entity<InputState>,
    bsky_password_input: Entity<InputState>,
    status: SharedString,
    loading: bool,
    cancellable: bool,
}

impl LoginView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let domain_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder("mastodon.social")
        });
        let bsky_id_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder("ユーザー名またはメールアドレス")
        });
        let bsky_password_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("アプリパスワード")
                .masked(true)
        });

        Self {
            domain_input,
            bsky_id_input,
            bsky_password_input,
            status: "".into(),
            loading: false,
            cancellable: false,
        }
    }

    pub fn cancellable(mut self, value: bool) -> Self {
        self.cancellable = value;
        self
    }

    fn start_login(&mut self, cx: &mut Context<Self>) {
        if self.loading {
            return;
        }

        let domain = self.domain_input.read(cx).value().to_string().trim().to_string();
        if domain.is_empty() {
            self.status = "Please enter an instance domain.".into();
            cx.notify();
            return;
        }

        let Some(app_state) = cx.try_global::<AppState>() else {
            self.status = "Database not available.".into();
            cx.notify();
            return;
        };
        let writer = app_state.database.writer().clone();

        self.loading = true;
        self.status = format!("Connecting to {}...", domain).into();
        cx.notify();

        let task = Tokio::spawn(cx, async move {
            run_login_flow(&domain, &writer).await
        });

        cx.spawn(async |this: WeakEntity<LoginView>, cx: &mut AsyncApp| {
            handle_login_result(this, cx, task.await).await;
        })
        .detach();
    }

    fn start_bluesky_login(&mut self, cx: &mut Context<Self>) {
        if self.loading {
            return;
        }

        let identifier = self
            .bsky_id_input
            .read(cx)
            .value()
            .to_string()
            .trim()
            .to_string();
        let password = self.bsky_password_input.read(cx).value().to_string();

        if identifier.is_empty() || password.is_empty() {
            self.status = "Bluesky のユーザー名とアプリパスワードを入力してください。".into();
            cx.notify();
            return;
        }

        self.loading = true;
        self.status = "Connecting to Bluesky...".into();
        cx.notify();

        let task = Tokio::spawn(cx, async move {
            run_bluesky_login(&identifier, &password).await
        });

        cx.spawn(async |this: WeakEntity<LoginView>, cx: &mut AsyncApp| {
            handle_login_result(this, cx, task.await).await;
        })
        .detach();
    }
}

async fn handle_login_result(
    this: WeakEntity<LoginView>,
    cx: &mut AsyncApp,
    result: Result<
        Result<(AccountSession, ServerKind), Box<dyn std::error::Error + Send + Sync>>,
        gpui_tokio_bridge::JoinError,
    >,
) {
    match result {
        Ok(Ok((session, kind))) => {
            let _ = this.update(cx, |this, cx| {
                this.loading = false;
                this.status = format!("Logged in as @{}", session.acct).into();
                cx.emit(LoginEvent::LoggedIn(session, kind));
                cx.notify();
            });
        }
        Ok(Err(e)) => {
            tracing::error!("Login failed: {}", e);
            let _ = this.update(cx, |this, cx| {
                this.loading = false;
                this.status = format!("Login failed: {}", e).into();
                cx.notify();
            });
        }
        Err(e) => {
            tracing::error!("Task error: {}", e);
            let _ = this.update(cx, |this, cx| {
                this.loading = false;
                this.status = format!("Error: {}", e).into();
                cx.notify();
            });
        }
    }
}

impl EventEmitter<LoginEvent> for LoginView {}

impl Render for LoginView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let loading = self.loading;

        div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(12.0))
            .bg(rgb(0x1e1e2e))
            .child(
                div()
                    .text_xl()
                    .text_color(rgb(0xcdd6f4))
                    .child("awayuki"),
            )
            .child(
                div()
                    .text_color(rgb(0x6c7086))
                    .child("Enter your instance domain to log in"),
            )
            .child(
                div()
                    .w(px(320.0))
                    .child(Input::new(&self.domain_input)),
            )
            .child(
                div().w(px(320.0)).child(
                    Button::new("login")
                        .label("Log in")
                        .loading(loading)
                        .on_click(cx.listener(|this, _event, _window, cx| {
                            this.start_login(cx);
                        })),
                ),
            )
            .child(separator_with_label("or"))
            .child(
                div()
                    .w(px(320.0))
                    .text_color(rgb(0xa6adc8))
                    .child("Bluesky:"),
            )
            .child(
                div()
                    .w(px(320.0))
                    .child(Input::new(&self.bsky_id_input)),
            )
            .child(
                div()
                    .w(px(320.0))
                    .child(Input::new(&self.bsky_password_input)),
            )
            .child(
                div().w(px(320.0)).child(
                    Button::new("bsky-login")
                        .label("Log in")
                        .loading(loading)
                        .on_click(cx.listener(|this, _event, _window, cx| {
                            this.start_bluesky_login(cx);
                        })),
                ),
            )
            .when(self.cancellable && !loading, |this| {
                this.child(separator())
                    .child(
                        div().w(px(320.0)).child(
                            Button::new("login-cancel")
                                .ghost()
                                .label("Cancel")
                                .on_click(cx.listener(|_this, _event, _window, cx| {
                                    cx.emit(LoginEvent::Cancelled);
                                })),
                        ),
                    )
            })
            .when(!self.status.is_empty(), |this| {
                this.child(
                    div()
                        .text_sm()
                        .text_color(rgb(0xa6adc8))
                        .child(self.status.clone()),
                )
            })
    }
}

fn separator() -> impl IntoElement {
    div()
        .w(px(320.0))
        .h(px(1.0))
        .my(px(4.0))
        .bg(rgb(0x313244))
}

fn separator_with_label(label: &'static str) -> impl IntoElement {
    div()
        .w(px(320.0))
        .my(px(4.0))
        .flex()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .child(div().flex_1().h(px(1.0)).bg(rgb(0x313244)))
        .child(
            div()
                .text_sm()
                .text_color(rgb(0x6c7086))
                .child(label),
        )
        .child(div().flex_1().h(px(1.0)).bg(rgb(0x313244)))
}

async fn run_login_flow(
    domain: &str,
    pool: &SqlitePool,
) -> Result<(AccountSession, ServerKind), Box<dyn std::error::Error + Send + Sync>> {
    let kind = detect_server_kind(domain).await?;
    tracing::info!("Detected server kind for {}: {:?}", domain, kind);
    match kind {
        ServerKind::Mastodon | ServerKind::Paon => run_mastodon_oauth(domain, pool, kind).await,
        ServerKind::Misskey => run_misskey_miauth(domain, kind).await,
        ServerKind::Bluesky => Err(
            "Bluesky cannot be configured via instance domain — use the Bluesky login form below."
                .into(),
        ),
    }
}

async fn run_mastodon_oauth(
    domain: &str,
    pool: &SqlitePool,
    kind: ServerKind,
) -> Result<(AccountSession, ServerKind), Box<dyn std::error::Error + Send + Sync>> {
    let port = callback_server::find_available_port().await?;

    let mut flow = OAuthFlow::new(domain, port)?;
    flow.prepare().await?;

    let auth_url = flow
        .authorize_url()
        .ok_or("Failed to generate authorization URL")?;

    let callback_handle = tokio::spawn(callback_server::wait_for_callback(port));

    tracing::info!("Opening browser for Mastodon authorization");
    open::that(&auth_url)?;

    let code = callback_handle.await??;

    let token_response = flow.exchange_code(&code).await?;
    tracing::info!("Got Mastodon access token");

    let instance = flow.instance.as_ref().ok_or("No instance info")?;
    let streaming_url = instance
        .streaming_url()
        .unwrap_or(&format!("wss://{}", domain))
        .to_string();

    let reg = flow.registration.as_ref().ok_or("No app registration")?;
    CredentialStore::save_client_credentials(pool, domain, &reg.client_id, &reg.client_secret).await?;

    let mastodon = MastodonClient::new(domain, token_response.access_token.clone(), streaming_url)?;
    let client = ApiClient::Mastodon(mastodon);

    let account = client.verify_credentials().await?;
    let acct = if account.acct.contains('@') {
        account.acct.clone()
    } else {
        format!("{}@{}", account.acct, domain)
    };
    tracing::info!("Mastodon login successful: @{}", acct);

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

async fn run_misskey_miauth(
    domain: &str,
    kind: ServerKind,
) -> Result<(AccountSession, ServerKind), Box<dyn std::error::Error + Send + Sync>> {
    let port = callback_server::find_available_port().await?;
    let flow = MiAuthFlow::new(domain, port)?;
    let auth_url = flow.authorize_url();

    let callback_handle = tokio::spawn(async move {
        callback_server::wait_for_callback_any(port, &["session", "code"]).await
    });

    tracing::info!("Opening browser for Misskey authorization");
    open::that(&auth_url)?;

    // We don't actually need the value Misskey returns — `flow.session_id` is what we use.
    let _ = callback_handle.await?;

    let result = flow.check().await?;
    tracing::info!("Got Misskey access token");

    let streaming_url = format!("wss://{}", domain);
    let misskey = MisskeyClient::new(domain, result.token.clone(), streaming_url)?;
    let client = ApiClient::Misskey(misskey);

    let account = client.verify_credentials().await?;
    let acct = if account.acct.contains('@') {
        account.acct.clone()
    } else {
        format!("{}@{}", account.acct, domain)
    };
    tracing::info!("Misskey login successful: @{}", acct);

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

async fn run_bluesky_login(
    identifier: &str,
    password: &str,
) -> Result<(AccountSession, ServerKind), Box<dyn std::error::Error + Send + Sync>> {
    let domain = DEFAULT_BLUESKY_HOST;
    let streaming_url = format!("wss://{}", domain);

    let bluesky = login_with_app_password(domain, identifier, password, streaming_url).await?;
    let client = ApiClient::Bluesky(bluesky);

    let account = client.verify_credentials().await?;
    let acct = if account.acct.contains('@') {
        account.acct.clone()
    } else {
        format!("{}@{}", account.acct, domain)
    };
    tracing::info!("Bluesky login successful: @{}", acct);

    Ok((
        AccountSession {
            acct,
            domain: domain.to_string(),
            client,
            account_info: account,
        },
        ServerKind::Bluesky,
    ))
}
