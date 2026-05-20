use crate::api::client::ApiClient;

/// The session currently selected for *user actions* — composing posts,
/// boosting, favouriting, bookmarking, voting in polls. In unified-timeline
/// mode this is decoupled from which account owns the timeline columns: the
/// columns stay pinned to the account that built the view, while this Global
/// tracks the account selected via the account switcher.
#[derive(Clone)]
pub struct ActiveAccount {
    pub client: ApiClient,
    pub acct: String,
    pub account_id: String,
}
