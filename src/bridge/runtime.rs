use gpui::App;

pub fn init_tokio_bridge(cx: &mut App) {
    gpui_tokio_bridge::init(cx);
}
