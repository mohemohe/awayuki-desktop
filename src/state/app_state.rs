use std::sync::Arc;

use gpui::Global;

use crate::db::pool::Database;

pub struct AppState {
    pub database: Arc<Database>,
}

impl Global for AppState {}
