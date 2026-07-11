//! Protocol-neutral names used at application port boundaries.
//!
//! These aliases preserve the released IPC/storage shape during the adapter
//! split. Removal condition: once all three adapters convert their wire DTOs
//! into owned domain structs, replace each alias here without changing port
//! method names or application use cases.

pub type Account = crate::mastodon::types::account::Account;
pub type CustomEmoji = crate::mastodon::types::account::CustomEmoji;
pub type Relationship = crate::mastodon::types::account::Relationship;
pub type AccountStatusesQuery = crate::mastodon::endpoints::accounts::AccountStatusesParams;
pub type NotificationQuery = crate::mastodon::endpoints::notifications::NotificationParams;
pub type StatusDraft = crate::mastodon::endpoints::statuses::CreateStatusParams;
pub type PollVote = crate::mastodon::endpoints::statuses::VotePollParams;
pub type TimelineQuery = crate::mastodon::endpoints::timelines::TimelineParams;
pub type Page<T> = crate::mastodon::client::PaginatedResponse<T>;
pub type List = crate::mastodon::types::list::List;
pub type Notification = crate::mastodon::types::notification::Notification;
pub type SearchResult = crate::mastodon::types::search::SearchResult;
pub type MediaAttachment = crate::mastodon::types::status::MediaAttachment;
pub type Poll = crate::mastodon::types::status::Poll;
pub type Status = crate::mastodon::types::status::Status;
pub type StatusContext = crate::mastodon::types::status::StatusContext;
