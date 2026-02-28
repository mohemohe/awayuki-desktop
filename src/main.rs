mod assets;
mod bridge;
mod constants;
mod db;
mod mastodon;
mod auth;
mod services;
mod state;
mod ui;
mod updater;

use gpui::prelude::*;
use gpui::{px, rgb, size, AnyView, App, Application, Bounds, WindowBounds, WindowOptions};
use gpui_component::Root;
use gpui_component::theme::Theme;
use tracing_subscriber::EnvFilter;

use crate::bridge::http::ReqwestHttpClient;
use crate::bridge::runtime::init_tokio_bridge;
use crate::constants::APP_NAME;
use crate::state::window_state;
use crate::ui::workspace::{FocusCompose, SubmitPost, Workspace};

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::from_default_env()
                .add_directive("awayuki=info".parse().unwrap()),
        )
        .init();

    tracing::info!("{} starting...", APP_NAME);

    Application::new()
        .with_assets(assets::CombinedAssets::new())
        .with_http_client(ReqwestHttpClient::new())
        .run(|cx: &mut App| {
        gpui_component::init(cx);
        // Customize theme to Catppuccin Mocha
        {
            let theme = Theme::global_mut(cx);
            theme.background = rgb(0x1e1e2e).into();          // Base
            theme.foreground = rgb(0xcdd6f4).into();           // Text
            theme.muted = rgb(0x313244).into();                // Surface0
            theme.muted_foreground = rgb(0xa6adc8).into();     // Subtext0
            theme.border = rgb(0x313244).into();               // Surface0
            theme.input = rgb(0x45475a).into();                // Surface1
            theme.ring = rgb(0x89b4fa).into();                 // Blue
            theme.primary = rgb(0x89b4fa).into();              // Blue
            theme.primary_hover = rgb(0x74c7ec).into();        // Sapphire
            theme.primary_active = rgb(0x89dceb).into();       // Sky
            theme.primary_foreground = rgb(0x1e1e2e).into();   // Base
            theme.secondary = rgb(0x313244).into();            // Surface0
            theme.secondary_hover = rgb(0x45475a).into();      // Surface1
            theme.secondary_active = rgb(0x585b70).into();     // Surface2
            theme.secondary_foreground = rgb(0xcdd6f4).into(); // Text
            theme.accent = rgb(0x45475a).into();               // Surface1 (select highlight)
            theme.accent_foreground = rgb(0xcdd6f4).into();    // Text
            theme.list_active = rgb(0x45475a).into();           // Surface1
            theme.list_active_border = rgb(0x89b4fa).into();    // Blue
        }
        init_tokio_bridge(cx);

        // Initialize Sparkle auto-updater
        updater::init_updater();

        // Register global key bindings
        cx.bind_keys([
            #[cfg(target_os = "macos")]
            gpui::KeyBinding::new("cmd-n", FocusCompose, None),
            #[cfg(target_os = "macos")]
            gpui::KeyBinding::new("cmd-enter", SubmitPost, None),
            #[cfg(not(target_os = "macos"))]
            gpui::KeyBinding::new("ctrl-n", FocusCompose, None),
            #[cfg(not(target_os = "macos"))]
            gpui::KeyBinding::new("ctrl-enter", SubmitPost, None),
        ]);

        let window_bounds = window_state::load_window_bounds().unwrap_or_else(|| {
            let bounds = Bounds::centered(None, size(px(1200.), px(800.)), cx);
            WindowBounds::Windowed(bounds)
        });
        cx.open_window(
            WindowOptions {
                window_bounds: Some(window_bounds),
                ..Default::default()
            },
            |window, cx| {
                window.on_window_should_close(cx, |window, _cx| {
                    window_state::save_window_bounds(window.window_bounds());
                    true
                });
                let workspace = cx.new(|cx| Workspace::new(window, cx));
                let view: AnyView = workspace.into();
                cx.new(|cx| Root::new(view, window, cx))
            },
        )
        .expect("Failed to open main window");
    });
}
