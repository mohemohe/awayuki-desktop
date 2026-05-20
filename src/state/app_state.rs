use std::sync::Arc;

use crate::db::pool::Database;

pub struct AppState {
    pub database: Arc<Database>,
}
