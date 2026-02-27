use gpui::prelude::*;
use gpui::{div, px, rgb, AsyncApp, Context, Entity, EventEmitter, SharedString, WeakEntity, Window};
use gpui_component::button::Button;
use gpui_component::input::{Input, InputState};
use gpui_tokio_bridge::Tokio;
use sqlx::SqlitePool;

use crate::auth::callback_server;
use crate::auth::credential_store::CredentialStore;
use crate::auth::session::AccountSession;
use crate::mastodon::client::MastodonClient;
use crate::mastodon::oauth::OAuthFlow;
use crate::state::app_state::AppState;

pub enum LoginEvent {
    LoggedIn(AccountSession),
}

pub struct LoginView {
    domain_input: Entity<InputState>,
    status: SharedString,
    loading: bool,
}

impl LoginView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let domain_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder("mastodon.social")
        });

        Self {
            domain_input,
            status: "".into(),
            loading: false,
        }
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
            run_oauth_flow(&domain, &writer).await
        });

        cx.spawn(async |this: WeakEntity<LoginView>, cx: &mut AsyncApp| {
            match task.await {
                Ok(Ok(session)) => {
                    let _ = this.update(cx, |this, cx| {
                        this.loading = false;
                        this.status = format!("Logged in as @{}", session.acct).into();
                        cx.emit(LoginEvent::LoggedIn(session));
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
        })
        .detach();
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
            .gap(px(16.0))
            .bg(rgb(0x1e1e2e))
            // Title
            .child(
                div()
                    .text_xl()
                    .text_color(rgb(0xcdd6f4))
                    .child("awayuki"),
            )
            // Subtitle
            .child(
                div()
                    .text_color(rgb(0x6c7086))
                    .child("Enter your instance domain to log in"),
            )
            // Domain input
            .child(
                div()
                    .w(px(320.0))
                    .child(Input::new(&self.domain_input)),
            )
            // Login button
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
            // Status message
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

async fn run_oauth_flow(domain: &str, pool: &SqlitePool) -> Result<AccountSession, Box<dyn std::error::Error + Send + Sync>> {
    // Find an available port for callback
    let port = callback_server::find_available_port().await?;

    // Prepare OAuth flow
    let mut flow = OAuthFlow::new(domain, port)?;
    flow.prepare().await?;

    let auth_url = flow
        .authorize_url()
        .ok_or("Failed to generate authorization URL")?;

    // Start callback server in background
    let callback_handle = tokio::spawn(callback_server::wait_for_callback(port));

    // Open browser
    tracing::info!("Opening browser for authorization");
    open::that(&auth_url)?;

    // Wait for callback
    let code = callback_handle.await??;

    // Exchange code for token
    let token_response = flow.exchange_code(&code).await?;
    tracing::info!("Got access token");

    // Get instance info
    let instance = flow.instance.as_ref().ok_or("No instance info")?;
    let streaming_url = instance
        .streaming_url()
        .unwrap_or(&format!("wss://{}", domain))
        .to_string();

    // Save client credentials to DB
    let reg = flow.registration.as_ref().ok_or("No app registration")?;
    CredentialStore::save_client_credentials(pool, domain, &reg.client_id, &reg.client_secret).await?;

    // Create authenticated client
    let client = MastodonClient::new(domain, token_response.access_token.clone(), streaming_url)?;

    // Verify credentials
    let account = client.verify_credentials().await?;
    let acct = if account.acct.contains('@') {
        account.acct.clone()
    } else {
        format!("{}@{}", account.acct, domain)
    };

    // Token is saved via upsert_login_account in on_login_success
    tracing::info!("Login successful: @{}", acct);

    Ok(AccountSession {
        acct,
        domain: domain.to_string(),
        client,
        account_info: account,
    })
}
