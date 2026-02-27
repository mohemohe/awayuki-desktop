pub const APP_NAME: &str = "awayuki";
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const APP_USER_AGENT: &str = concat!("awayuki/", env!("CARGO_PKG_VERSION"));
pub const DB_FILENAME: &str = "awayuki.db";
pub const DEFAULT_COLUMN_WIDTH: u32 = 350;
pub const DEFAULT_TIMELINE_LIMIT: u32 = 40;
