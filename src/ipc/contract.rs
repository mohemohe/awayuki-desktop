#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandKind {
    Read,
    Mutation,
}

impl std::fmt::Display for CommandKind {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Read => "read",
            Self::Mutation => "mutation",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancelPolicy {
    Unsupported,
    Cooperative,
}

impl std::fmt::Display for CancelPolicy {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Unsupported => "unsupported",
            Self::Cooperative => "cooperative",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Capability {
    AccountRead,
    AccountWrite,
    ApplicationRead,
    Authentication,
    DiagnosticsRead,
    ExternalWrite,
    Maintenance,
    MediaWrite,
    NotificationRead,
    NotificationWrite,
    RelationshipWrite,
    SettingsWrite,
    SidecarWrite,
    StatusWrite,
    SuggestionRead,
    SupportRead,
    TimelineRead,
    Translation,
}

impl std::fmt::Display for Capability {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::AccountRead => "account.read",
            Self::AccountWrite => "account.write",
            Self::ApplicationRead => "application.read",
            Self::Authentication => "authentication",
            Self::DiagnosticsRead => "diagnostics.read",
            Self::ExternalWrite => "external.write",
            Self::Maintenance => "maintenance",
            Self::MediaWrite => "media.write",
            Self::NotificationRead => "notification.read",
            Self::NotificationWrite => "notification.write",
            Self::RelationshipWrite => "relationship.write",
            Self::SettingsWrite => "settings.write",
            Self::SidecarWrite => "sidecar.write",
            Self::StatusWrite => "status.write",
            Self::SuggestionRead => "suggestion.read",
            Self::SupportRead => "support.read",
            Self::TimelineRead => "timeline.read",
            Self::Translation => "translation",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommandMetadata {
    pub name: &'static str,
    pub kind: CommandKind,
    pub timeout_ms: u32,
    pub cancel: CancelPolicy,
    pub capability: Capability,
    pub args_type: &'static str,
    pub result_type: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SettingMetadata {
    pub key: &'static str,
    pub schema_version: u32,
    pub rust_type: &'static str,
    pub snapshot_field: &'static str,
}

pub const SETTINGS_SCHEMA_VERSION: u32 = 1;

pub const SETTINGS: &[SettingMetadata] = &[
    SettingMetadata {
        key: "appearance",
        schema_version: 1,
        rust_type: "AppearanceSettings",
        snapshot_field: "appearance",
    },
    SettingMetadata {
        key: "performance",
        schema_version: 1,
        rust_type: "PerformanceSettings",
        snapshot_field: "performance",
    },
    SettingMetadata {
        key: "confirmation",
        schema_version: 1,
        rust_type: "ConfirmationSettings",
        snapshot_field: "confirmation",
    },
    SettingMetadata {
        key: "bluesky_fetch",
        schema_version: 1,
        rust_type: "BlueskyFetchSettings",
        snapshot_field: "blueskyFetch",
    },
    SettingMetadata {
        key: "sidecars",
        schema_version: 1,
        rust_type: "SidecarSettings",
        snapshot_field: "sidecars",
    },
    SettingMetadata {
        key: "account_source_colors",
        schema_version: 1,
        rust_type: "Record<string, AccountSourceColor>",
        snapshot_field: "accountSourceColors",
    },
    SettingMetadata {
        key: "preset_visibility",
        schema_version: 1,
        rust_type: "PresetVisibilitySettings",
        snapshot_field: "presetVisibility",
    },
    SettingMetadata {
        key: "debug",
        schema_version: 1,
        rust_type: "DebugSettings",
        snapshot_field: "debug",
    },
    SettingMetadata {
        key: "notification_suppression",
        schema_version: 1,
        rust_type: "NotificationSuppressionList",
        snapshot_field: "notificationSuppression",
    },
];

const fn command(
    name: &'static str,
    kind: CommandKind,
    timeout_ms: u32,
    capability: Capability,
    args_type: &'static str,
    result_type: &'static str,
) -> CommandMetadata {
    CommandMetadata {
        name,
        kind,
        timeout_ms,
        // Tauri IPC currently has no caller-visible cancellation token. Keep
        // this truthful until cooperative cancellation is wired end-to-end.
        cancel: CancelPolicy::Unsupported,
        capability,
        args_type,
        result_type,
    }
}

const fn cancellable_command(
    name: &'static str,
    kind: CommandKind,
    timeout_ms: u32,
    capability: Capability,
    args_type: &'static str,
    result_type: &'static str,
) -> CommandMetadata {
    CommandMetadata {
        name,
        kind,
        timeout_ms,
        cancel: CancelPolicy::Cooperative,
        capability,
        args_type,
        result_type,
    }
}

use Capability as Cap;
use CommandKind::{Mutation, Read};

/// Canonical metadata for every command registered with Tauri.
///
/// `Read` is the SAFE-01 classification: it is observational and may be
/// retried after a response-transport failure. Every other command is a
/// `Mutation` and must never be resent automatically.
pub const COMMANDS: &[CommandMetadata] = &[
    command(
        "app_snapshot",
        Read,
        600_000,
        Cap::ApplicationRead,
        "NoArgs",
        "AppSnapshot",
    ),
    command(
        "report_release_webview_smoke",
        Mutation,
        5_000,
        Cap::DiagnosticsRead,
        "ReleaseWebviewSmokeReport",
        "Unit",
    ),
    command(
        "start_runtime_initialization",
        Mutation,
        5_000,
        Cap::ApplicationRead,
        "NoArgs",
        "Unit",
    ),
    command(
        "retry_runtime_initialization",
        Mutation,
        5_000,
        Cap::ApplicationRead,
        "NoArgs",
        "Unit",
    ),
    command(
        "account_summaries",
        Read,
        5_000,
        Cap::AccountRead,
        "NoArgs",
        "Vec<AccountSummary>",
    ),
    command(
        "account_lists",
        Read,
        30_000,
        Cap::AccountRead,
        "AccountListsRequest",
        "Vec<AccountListSummary>",
    ),
    cancellable_command(
        "login_with_instance_domain",
        Mutation,
        120_000,
        Cap::Authentication,
        "LoginInstanceRequest",
        "AppSnapshot",
    ),
    cancellable_command(
        "login_with_bluesky_app_password",
        Mutation,
        60_000,
        Cap::Authentication,
        "LoginBlueskyRequest",
        "AppSnapshot",
    ),
    command(
        "cancel_login_flow",
        Mutation,
        5_000,
        Cap::Authentication,
        "CancelLoginFlowRequest",
        "bool",
    ),
    cancellable_command(
        "load_timeline",
        Read,
        30_000,
        Cap::TimelineRead,
        "TimelineRequest",
        "Vec<TimelineStatus>",
    ),
    cancellable_command(
        "load_more_timeline",
        Read,
        30_000,
        Cap::TimelineRead,
        "TimelineRequest",
        "TimelinePageResponse",
    ),
    command(
        "cancel_timeline_query",
        Mutation,
        5_000,
        Cap::TimelineRead,
        "CancelTimelineQueryRequest",
        "bool",
    ),
    command(
        "cancel_quote_consumer",
        Mutation,
        5_000,
        Cap::TimelineRead,
        "CancelQuoteConsumerRequest",
        "bool",
    ),
    command(
        "cancel_mutation_operation",
        Mutation,
        5_000,
        Cap::StatusWrite,
        "CancelMutationOperationRequest",
        "bool",
    ),
    cancellable_command(
        "refresh_timeline",
        Read,
        60_000,
        Cap::TimelineRead,
        "TimelineRequest",
        "Vec<TimelineStatus>",
    ),
    command(
        "status_viewer_states",
        Read,
        5_000,
        Cap::TimelineRead,
        "StatusViewerStatesRequest",
        "Vec<StatusViewerStateSummary>",
    ),
    command(
        "status_thread",
        Read,
        30_000,
        Cap::TimelineRead,
        "StatusThreadRequest",
        "Vec<TimelineStatus>",
    ),
    command(
        "air_context",
        Read,
        30_000,
        Cap::TimelineRead,
        "AirContextRequest",
        "Vec<TimelineStatus>",
    ),
    cancellable_command(
        "account_profile",
        Read,
        30_000,
        Cap::AccountRead,
        "AccountProfileRequest",
        "AccountProfileSummary",
    ),
    cancellable_command(
        "account_timeline",
        Read,
        30_000,
        Cap::AccountRead,
        "AccountTimelineRequest",
        "Vec<TimelineStatus>",
    ),
    command(
        "account_follow_action",
        Mutation,
        30_000,
        Cap::RelationshipWrite,
        "AccountFollowRequest",
        "AccountRelationshipSummary",
    ),
    command(
        "notification_muted_accounts",
        Read,
        10_000,
        Cap::NotificationRead,
        "NoArgs",
        "Vec<NotificationMutedAccountSummary>",
    ),
    command(
        "set_account_notification_mute",
        Mutation,
        10_000,
        Cap::NotificationWrite,
        "AccountNotificationMuteRequest",
        "bool",
    ),
    command(
        "post_status",
        Mutation,
        60_000,
        Cap::StatusWrite,
        "PostRequest",
        "TimelineStatus",
    ),
    command(
        "begin_compose_media_upload",
        Mutation,
        10_000,
        Cap::MediaWrite,
        "BeginMediaUploadRequest",
        "BeginMediaUploadResponse",
    ),
    command(
        "append_compose_media_upload",
        Mutation,
        30_000,
        Cap::MediaWrite,
        "RawMediaChunkHeaders",
        "MediaUploadProgressResponse",
    ),
    command(
        "finish_compose_media_upload",
        Mutation,
        120_000,
        Cap::MediaWrite,
        "MediaUploadIdRequest",
        "MediaAttachment",
    ),
    command(
        "cancel_compose_media_upload",
        Mutation,
        10_000,
        Cap::MediaWrite,
        "MediaUploadIdRequest",
        "()",
    ),
    command(
        "claim_dropped_media_path",
        Mutation,
        5_000,
        Cap::MediaWrite,
        "ClaimDroppedMediaPathRequest",
        "ClaimDroppedMediaPathResponse",
    ),
    command(
        "upload_compose_media_path",
        Mutation,
        120_000,
        Cap::MediaWrite,
        "UploadMediaPathRequest",
        "MediaAttachment",
    ),
    cancellable_command(
        "autocomplete_mentions",
        Read,
        15_000,
        Cap::SuggestionRead,
        "ComposeSuggestionRequest",
        "Vec<MentionSuggestionView>",
    ),
    cancellable_command(
        "autocomplete_hashtags",
        Read,
        15_000,
        Cap::SuggestionRead,
        "ComposeSuggestionRequest",
        "Vec<HashtagSuggestionView>",
    ),
    command(
        "custom_emojis",
        Read,
        30_000,
        Cap::SuggestionRead,
        "NoArgs",
        "Vec<CustomEmojiView>",
    ),
    command(
        "edit_own_status",
        Mutation,
        60_000,
        Cap::StatusWrite,
        "EditStatusRequest",
        "TimelineStatus",
    ),
    command(
        "delete_own_status",
        Mutation,
        30_000,
        Cap::StatusWrite,
        "DeleteStatusRequest",
        "()",
    ),
    command(
        "vote_poll",
        Mutation,
        30_000,
        Cap::StatusWrite,
        "VotePollRequest",
        "PollView",
    ),
    command(
        "switch_active_account",
        Mutation,
        15_000,
        Cap::AccountWrite,
        "AccountHandleArgs",
        "AppSnapshot",
    ),
    command(
        "logout_account",
        Mutation,
        30_000,
        Cap::AccountWrite,
        "AccountHandleArgs",
        "AppSnapshot",
    ),
    command(
        "save_settings",
        Mutation,
        15_000,
        Cap::SettingsWrite,
        "SaveSettingsRequest",
        "SettingsSnapshot",
    ),
    command(
        "translate_status_text",
        Mutation,
        60_000,
        Cap::Translation,
        "TranslateStatusRequest",
        "TranslateStatusResponse",
    ),
    command(
        "save_columns",
        Mutation,
        15_000,
        Cap::SettingsWrite,
        "SaveColumnsRequest",
        "AppSnapshot",
    ),
    command(
        "explain_custom_timeline",
        Read,
        5_000,
        Cap::TimelineRead,
        "ExplainCustomTimelineRequest",
        "Vec<QueryPlanStep>",
    ),
    command(
        "vacuum_database",
        Mutation,
        120_000,
        Cap::Maintenance,
        "NoArgs",
        "DbSummary",
    ),
    command(
        "clear_status_cache",
        Mutation,
        60_000,
        Cap::Maintenance,
        "NoArgs",
        "DbSummary",
    ),
    command(
        "status_bar_snapshot",
        Read,
        5_000,
        Cap::ApplicationRead,
        "NoArgs",
        "StatusBarSnapshot",
    ),
    command(
        "status_action",
        Mutation,
        30_000,
        Cap::StatusWrite,
        "StatusActionRequest",
        "TimelineStatus",
    ),
    cancellable_command(
        "download_media",
        Mutation,
        120_000,
        Cap::MediaWrite,
        "DownloadMediaRequest",
        "()",
    ),
    command(
        "cancel_media_download",
        Mutation,
        5_000,
        Cap::MediaWrite,
        "CancelMediaDownloadRequest",
        "bool",
    ),
    command(
        "open_status_url",
        Mutation,
        5_000,
        Cap::ExternalWrite,
        "UrlArgs",
        "()",
    ),
    command(
        "create_sidecar_webview",
        Mutation,
        15_000,
        Cap::SidecarWrite,
        "CreateSidecarWebviewRequest",
        "()",
    ),
    command(
        "navigate_sidecar_webview",
        Mutation,
        15_000,
        Cap::SidecarWrite,
        "NavigateSidecarWebviewArgs",
        "()",
    ),
    command(
        "reload_sidecar_webview",
        Mutation,
        15_000,
        Cap::SidecarWrite,
        "SidecarIdArgs",
        "()",
    ),
    command(
        "close_sidecar_webview",
        Mutation,
        15_000,
        Cap::SidecarWrite,
        "SidecarIdArgs",
        "()",
    ),
    command(
        "scroll_sidecar_webview_to_top",
        Mutation,
        15_000,
        Cap::SidecarWrite,
        "SidecarIdArgs",
        "()",
    ),
    command(
        "inject_sidecar_user_style",
        Mutation,
        15_000,
        Cap::SidecarWrite,
        "InjectSidecarUserStyleArgs",
        "()",
    ),
    command(
        "open_log_file",
        Mutation,
        5_000,
        Cap::ExternalWrite,
        "NoArgs",
        "()",
    ),
    command(
        "diagnostics_snapshot",
        Read,
        5_000,
        Cap::DiagnosticsRead,
        "NoArgs",
        "DiagnosticsSnapshot",
    ),
    command(
        "support_bundle",
        Read,
        30_000,
        Cap::SupportRead,
        "SupportBundleRequest",
        "SupportBundle",
    ),
];

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    fn registered_tauri_commands() -> BTreeSet<&'static str> {
        let source = include_str!("../application/desktop.rs");
        let marker = "tauri::generate_handler![";
        let commands = source
            .split_once(marker)
            .expect("Tauri handler registry must exist")
            .1
            .split_once(']')
            .expect("Tauri handler registry must be closed")
            .0;
        commands
            .split(',')
            .map(str::trim)
            .map(|command| command.rsplit("::").next().unwrap_or(command))
            .filter(|command| !command.is_empty())
            .collect()
    }

    #[test]
    fn metadata_is_complete_for_the_tauri_handler_registry() {
        let registered = registered_tauri_commands();
        let metadata = COMMANDS
            .iter()
            .map(|command| command.name)
            .collect::<BTreeSet<_>>();
        assert_eq!(metadata, registered);
        assert_eq!(
            metadata.len(),
            COMMANDS.len(),
            "command names must be unique"
        );
    }

    #[test]
    fn every_command_has_actionable_metadata() {
        for command in COMMANDS {
            assert!(command.timeout_ms > 0, "{} has no timeout", command.name);
            assert!(
                !command.args_type.is_empty(),
                "{} has no args type",
                command.name
            );
            assert!(
                !command.result_type.is_empty(),
                "{} has no result type",
                command.name
            );
            assert!(command
                .name
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_'));
        }
    }
}
