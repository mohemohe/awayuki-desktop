use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TimelineDisplayFilter {
    #[serde(default)]
    pub(crate) enabled: bool,
    #[serde(default)]
    pub(crate) exclude_boosts: bool,
    #[serde(default)]
    pub(crate) exclude_media: bool,
    #[serde(default)]
    pub(crate) include_media: bool,
}

#[allow(dead_code)] // The generator target uses metadata but not runtime behavior.
impl TimelineDisplayFilter {
    pub(crate) fn applies(self) -> bool {
        self.enabled && (self.exclude_boosts || self.exclude_media || self.include_media)
    }
}

#[derive(Debug, Clone, Copy)]
#[allow(dead_code)] // Consumed by the standalone contract generator target.
pub struct DtoFieldMetadata {
    pub rust_name: &'static str,
    pub serialized_name: &'static str,
    pub ts_type: &'static str,
    pub optional: bool,
}

#[derive(Debug, Clone, Copy)]
#[allow(dead_code)] // Consumed by the standalone contract generator target.
pub struct DtoMetadata {
    pub name: &'static str,
    pub fields: &'static [DtoFieldMetadata],
}

#[derive(Debug, Clone, Copy)]
#[allow(dead_code)] // Consumed by the standalone contract generator target.
pub struct TypedCommandMetadata {
    pub name: &'static str,
    pub args_type: &'static str,
    pub result_type: &'static str,
}

#[derive(Debug, Clone, Copy)]
#[allow(dead_code)] // Consumed by the standalone contract generator target.
pub struct RawCommandMetadata {
    pub name: &'static str,
    pub result_type: &'static str,
}

macro_rules! rust_field_type {
    (required_bool) => {
        bool
    };
    (required_string) => {
        String
    };
    (optional_string) => {
        Option<String>
    };
    (optional_u32) => {
        Option<u32>
    };
    (optional_bool) => {
        Option<bool>
    };
    (optional_timeline_display_filter) => {
        Option<TimelineDisplayFilter>
    };
    (required_status_identity) => {
        crate::domain::identity::StatusIdentity
    };
    (optional_status_identity) => {
        Option<crate::domain::identity::StatusIdentity>
    };
    (required_status_identity_vec) => {
        Vec<crate::domain::identity::StatusIdentity>
    };
    (required_i64_vec) => {
        Vec<i64>
    };
    (required_i64) => {
        i64
    };
    (required_u64) => {
        u64
    };
    (required_u32) => {
        u32
    };
    (required_i32) => {
        i32
    };
    (required_f64) => {
        f64
    };
    (required_string_vec) => {
        Vec<String>
    };
    (optional_string_vec) => {
        Option<Vec<String>>
    };
    (optional_post_poll) => {
        Option<PostPollRequest>
    };
    (required_json) => {
        serde_json::Value
    };
    (optional_translation_engine) => {
        Option<crate::state::confirmation::TranslationEngine>
    };
    (required_column_vec) => {
        Vec<ColumnSummary>
    };
}

macro_rules! ts_field_type {
    (required_bool) => {
        "boolean"
    };
    (required_string) => {
        "string"
    };
    (optional_string) => {
        "string | null"
    };
    (optional_u32) => {
        "number | null"
    };
    (optional_bool) => {
        "boolean | null"
    };
    (optional_timeline_display_filter) => {
        "TimelineDisplayFilter | null"
    };
    (required_status_identity) => {
        "StatusIdentity"
    };
    (optional_status_identity) => {
        "StatusIdentity | null"
    };
    (required_status_identity_vec) => {
        "StatusIdentity[]"
    };
    (required_i64_vec) => {
        "number[]"
    };
    (required_i64) => {
        "number"
    };
    (required_u64) => {
        "number"
    };
    (required_u32) => {
        "number"
    };
    (required_i32) => {
        "number"
    };
    (required_f64) => {
        "number"
    };
    (required_string_vec) => {
        "string[]"
    };
    (optional_string_vec) => {
        "string[] | null"
    };
    (optional_post_poll) => {
        "PostPollRequest | null"
    };
    (required_json) => {
        "unknown"
    };
    (optional_translation_engine) => {
        "\"FoundationModel\" | \"TranslationFramework\" | null"
    };
    (required_column_vec) => {
        "ColumnSummary[]"
    };
}

macro_rules! field_optional {
    (required_bool) => {
        false
    };
    (required_string) => {
        false
    };
    (optional_string) => {
        true
    };
    (optional_u32) => {
        true
    };
    (optional_bool) => {
        true
    };
    (optional_timeline_display_filter) => {
        true
    };
    (required_status_identity) => {
        false
    };
    (optional_status_identity) => {
        true
    };
    (required_status_identity_vec) => {
        false
    };
    (required_i64_vec) => {
        false
    };
    (required_i64) => {
        false
    };
    (required_u64) => {
        false
    };
    (required_u32) => {
        false
    };
    (required_i32) => {
        false
    };
    (required_f64) => {
        false
    };
    (required_string_vec) => {
        false
    };
    (optional_string_vec) => {
        true
    };
    (optional_post_poll) => {
        true
    };
    (required_json) => {
        false
    };
    (optional_translation_engine) => {
        true
    };
    (required_column_vec) => {
        false
    };
}

macro_rules! ipc_dto {
    (
        $module:ident => $name:ident {
            $( $rust_name:ident ($serialized_name:literal): $kind:ident ),* $(,)?
        }
    ) => {
        #[derive(Debug, Clone, Serialize, Deserialize)]
        #[serde(rename_all = "camelCase")]
        #[allow(dead_code)] // The generator includes metadata without constructing runtime DTOs.
        pub(crate) struct $name {
            $(pub(crate) $rust_name: rust_field_type!($kind)),*
        }

        mod $module {
            use super::{DtoFieldMetadata, DtoMetadata};

            #[allow(dead_code)] // Consumed by the standalone contract generator target.
            pub const METADATA: DtoMetadata = DtoMetadata {
                name: stringify!($name),
                fields: &[
                    $(DtoFieldMetadata {
                        rust_name: stringify!($rust_name),
                        serialized_name: $serialized_name,
                        ts_type: ts_field_type!($kind),
                        optional: field_optional!($kind),
                    }),*
                ],
            };
        }
    };
}

ipc_dto!(login_instance_request => LoginInstanceRequest {
    operation_id("operationId"): optional_string,
    domain("domain"): required_string,
});

ipc_dto!(login_bluesky_request => LoginBlueskyRequest {
    operation_id("operationId"): optional_string,
    identifier("identifier"): required_string,
    password("password"): required_string,
});

ipc_dto!(cancel_login_flow_request => CancelLoginFlowRequest {
    operation_id("operationId"): optional_string,
    target_operation_id("targetOperationId"): required_string,
});

ipc_dto!(download_media_request => DownloadMediaRequest {
    operation_id("operationId"): optional_string,
    url("url"): required_string,
    suggested_filename("suggestedFilename"): optional_string,
});

ipc_dto!(cancel_media_download_request => CancelMediaDownloadRequest {
    operation_id("operationId"): optional_string,
    target_operation_id("targetOperationId"): required_string,
});

ipc_dto!(cancel_timeline_query_request => CancelTimelineQueryRequest {
    operation_id("operationId"): optional_string,
    target_operation_id("targetOperationId"): required_string,
});

ipc_dto!(cancel_quote_consumer_request => CancelQuoteConsumerRequest {
    operation_id("operationId"): optional_string,
    quote_consumer_id("quoteConsumerId"): required_string,
});

ipc_dto!(cancel_mutation_operation_request => CancelMutationOperationRequest {
    operation_id("operationId"): optional_string,
    target_operation_id("targetOperationId"): required_string,
});

ipc_dto!(release_webview_smoke_report => ReleaseWebviewSmokeReport {
    image_loaded("imageLoaded"): required_bool,
    protocol_media_loaded("protocolMediaLoaded"): required_bool,
    custom_emoji_loaded("customEmojiLoaded"): required_bool,
    video_loaded("videoLoaded"): required_bool,
    sidecar_created("sidecarCreated"): required_bool,
    sidecar_hidden_during_preview("sidecarHiddenDuringPreview"): required_bool,
    sidecar_restored("sidecarRestored"): required_bool,
    sidecar_closed("sidecarClosed"): required_bool,
    csp_violation_count("cspViolationCount"): required_u32,
});

ipc_dto!(timeline_request => TimelineRequest {
    operation_id("operationId"): optional_string,
    column_type("columnType"): required_string,
    column_param("columnParam"): optional_string,
    limit("limit"): optional_u32,
    offset("offset"): optional_u32,
    max_status_id("maxStatusId"): optional_string,
    max_server_domain("maxServerDomain"): optional_string,
    since_status_id("sinceStatusId"): optional_string,
    since_server_domain("sinceServerDomain"): optional_string,
    account_acct("accountAcct"): optional_string,
    acting_account_acct("actingAccountAcct"): optional_string,
    display_filter("displayFilter"): optional_timeline_display_filter,
    quote_consumer_id("quoteConsumerId"): optional_string,
});

ipc_dto!(account_lists_request => AccountListsRequest {
    acct("acct"): required_string,
});

ipc_dto!(account_feeds_request => AccountFeedsRequest {
    acct("acct"): required_string,
});

ipc_dto!(account_profile_request => AccountProfileRequest {
    operation_id("operationId"): optional_string,
    account_id("accountId"): required_string,
    server_domain("serverDomain"): required_string,
    source_acct("sourceAcct"): optional_string,
});

ipc_dto!(account_timeline_request => AccountTimelineRequest {
    operation_id("operationId"): optional_string,
    account_id("accountId"): required_string,
    server_domain("serverDomain"): required_string,
    source_acct("sourceAcct"): optional_string,
    only_media("onlyMedia"): optional_bool,
    pinned("pinned"): optional_bool,
    limit("limit"): optional_u32,
    offset("offset"): optional_u32,
    cursor("cursor"): optional_string,
    quote_consumer_id("quoteConsumerId"): optional_string,
});

ipc_dto!(account_follow_request => AccountFollowRequest {
    account_id("accountId"): required_string,
    server_domain("serverDomain"): required_string,
    target_acct("targetAcct"): required_string,
    acting_account_acct("actingAccountAcct"): required_string,
    action("action"): required_string,
});

ipc_dto!(account_notification_mute_request => AccountNotificationMuteRequest {
    account_id("accountId"): required_string,
    server_domain("serverDomain"): required_string,
    muted("muted"): required_bool,
});

ipc_dto!(status_action_request => StatusActionRequest {
    identity("identity"): required_status_identity,
    acting_account_acct("actingAccountAcct"): required_string,
    action("action"): required_string,
});

ipc_dto!(vote_poll_request => VotePollRequest {
    identity("identity"): required_status_identity,
    acting_account_acct("actingAccountAcct"): required_string,
    poll_id("pollId"): required_string,
    choices("choices"): required_i64_vec,
});

ipc_dto!(edit_status_request => EditStatusRequest {
    identity("identity"): required_status_identity,
    acting_account_acct("actingAccountAcct"): required_string,
    account_id("accountId"): required_string,
    status("status"): required_string,
    visibility("visibility"): optional_string,
    spoiler_text("spoilerText"): optional_string,
    sensitive("sensitive"): optional_bool,
});

ipc_dto!(delete_status_request => DeleteStatusRequest {
    identity("identity"): required_status_identity,
    acting_account_acct("actingAccountAcct"): required_string,
    account_id("accountId"): required_string,
});

ipc_dto!(post_poll_request => PostPollRequest {
    options("options"): required_string_vec,
    multiple("multiple"): required_bool,
    expires_in("expiresIn"): required_i64,
});

ipc_dto!(post_request => PostRequest {
    operation_id("operationId"): optional_string,
    acting_account_acct("actingAccountAcct"): required_string,
    status("status"): required_string,
    visibility("visibility"): optional_string,
    spoiler_text("spoilerText"): optional_string,
    sensitive("sensitive"): optional_bool,
    media_ids("mediaIds"): optional_string_vec,
    in_reply_to_id("inReplyToId"): optional_string,
    in_reply_to_identity("inReplyToIdentity"): optional_status_identity,
    quote_id("quoteId"): optional_string,
    quote_identity("quoteIdentity"): optional_status_identity,
    poll("poll"): optional_post_poll,
});

ipc_dto!(compose_outbox_item_request => ComposeOutboxItemRequest {
    id("id"): required_string,
});

ipc_dto!(begin_media_upload_request => BeginMediaUploadRequest {
    acting_account_acct("actingAccountAcct"): required_string,
    filename("filename"): required_string,
    mime_type("mimeType"): required_string,
    size("size"): required_u64,
});

ipc_dto!(media_upload_id_request => MediaUploadIdRequest {
    upload_id("uploadId"): required_string,
});

ipc_dto!(claim_dropped_media_path_request => ClaimDroppedMediaPathRequest {
    path("path"): required_string,
});

ipc_dto!(upload_media_path_request => UploadMediaPathRequest {
    acting_account_acct("actingAccountAcct"): required_string,
    path("path"): required_string,
    capability("capability"): required_string,
});

ipc_dto!(compose_suggestion_request => ComposeSuggestionRequest {
    operation_id("operationId"): optional_string,
    query("query"): required_string,
    limit("limit"): optional_u32,
    account_acct("accountAcct"): optional_string,
});

ipc_dto!(save_settings_request => SaveSettingsRequest {
    key("key"): required_string,
    value("value"): required_json,
});

ipc_dto!(plugin_id_request => PluginIdRequest {
    plugin_id("pluginId"): required_string,
});

ipc_dto!(plugin_compose_button_request => PluginComposeButtonRequest {
    plugin_id("pluginId"): required_string,
    button_id("buttonId"): required_string,
    generation("generation"): required_u64,
    compose("compose"): required_json,
});

ipc_dto!(translate_status_request => TranslateStatusRequest {
    text("text"): required_string,
    source_language("sourceLanguage"): optional_string,
    target_language("targetLanguage"): required_string,
    translation_engine("translationEngine"): optional_translation_engine,
});

ipc_dto!(column_summary => ColumnSummary {
    id("id"): required_string,
    column_type("columnType"): required_string,
    column_param("columnParam"): optional_string,
    name("name"): required_string,
    max_statuses("maxStatuses"): required_u32,
    pane_index("paneIndex"): required_u32,
    position("position"): required_i32,
    account_acct("accountAcct"): optional_string,
    display_filter("displayFilter"): optional_timeline_display_filter,
    desktop_notifications("desktopNotifications"): optional_bool,
    notification_sound("notificationSound"): optional_string,
});

ipc_dto!(save_columns_request => SaveColumnsRequest {
    columns("columns"): required_column_vec,
});

ipc_dto!(create_sidecar_webview_request => CreateSidecarWebviewRequest {
    sidecar_id("sidecarId"): required_string,
    url("url"): required_string,
    user_style("userStyle"): required_string,
    x("x"): required_f64,
    y("y"): required_f64,
    width("width"): required_f64,
    height("height"): required_f64,
});

ipc_dto!(status_viewer_states_request => StatusViewerStatesRequest {
    operation_id("operationId"): optional_string,
    acting_account_acct("actingAccountAcct"): required_string,
    identities("identities"): required_status_identity_vec,
});

ipc_dto!(status_thread_request => StatusThreadRequest {
    status_id("statusId"): required_string,
    server_domain("serverDomain"): required_string,
    source_acct("sourceAcct"): optional_string,
    limit("limit"): optional_u32,
    quote_consumer_id("quoteConsumerId"): optional_string,
});

ipc_dto!(air_context_request => AirContextRequest {
    status_id("statusId"): required_string,
    server_domain("serverDomain"): required_string,
    source_acct("sourceAcct"): optional_string,
    account_id("accountId"): required_string,
    account_acct("accountAcct"): optional_string,
    notification_created_at("notificationCreatedAt"): required_string,
    limit("limit"): optional_u32,
    quote_consumer_id("quoteConsumerId"): optional_string,
});

ipc_dto!(explain_custom_timeline_request => ExplainCustomTimelineRequest {
    sql("sql"): required_string,
    operation_id("operationId"): optional_string,
});

ipc_dto!(icu_match_expression_request => IcuMatchExpressionRequest {
    term("term"): required_string,
});

#[allow(dead_code)] // Consumed by the standalone contract generator target.
pub const DTOS: &[DtoMetadata] = &[
    login_instance_request::METADATA,
    login_bluesky_request::METADATA,
    cancel_login_flow_request::METADATA,
    download_media_request::METADATA,
    cancel_media_download_request::METADATA,
    cancel_timeline_query_request::METADATA,
    cancel_quote_consumer_request::METADATA,
    cancel_mutation_operation_request::METADATA,
    release_webview_smoke_report::METADATA,
    timeline_request::METADATA,
    account_lists_request::METADATA,
    account_feeds_request::METADATA,
    account_profile_request::METADATA,
    account_timeline_request::METADATA,
    account_follow_request::METADATA,
    account_notification_mute_request::METADATA,
    status_action_request::METADATA,
    vote_poll_request::METADATA,
    edit_status_request::METADATA,
    delete_status_request::METADATA,
    post_poll_request::METADATA,
    post_request::METADATA,
    compose_outbox_item_request::METADATA,
    begin_media_upload_request::METADATA,
    media_upload_id_request::METADATA,
    claim_dropped_media_path_request::METADATA,
    upload_media_path_request::METADATA,
    compose_suggestion_request::METADATA,
    save_settings_request::METADATA,
    plugin_id_request::METADATA,
    plugin_compose_button_request::METADATA,
    translate_status_request::METADATA,
    column_summary::METADATA,
    save_columns_request::METADATA,
    create_sidecar_webview_request::METADATA,
    status_viewer_states_request::METADATA,
    status_thread_request::METADATA,
    air_context_request::METADATA,
    explain_custom_timeline_request::METADATA,
    icu_match_expression_request::METADATA,
];

#[allow(dead_code)] // Consumed by the standalone contract generator target.
pub const TYPED_COMMANDS: &[TypedCommandMetadata] = &[
    TypedCommandMetadata {
        name: "login_with_instance_domain",
        args_type: "{ request: LoginInstanceRequest }",
        result_type: "AppSnapshot",
    },
    TypedCommandMetadata {
        name: "login_with_bluesky_app_password",
        args_type: "{ request: LoginBlueskyRequest }",
        result_type: "AppSnapshot",
    },
    TypedCommandMetadata {
        name: "cancel_login_flow",
        args_type: "{ request: CancelLoginFlowRequest }",
        result_type: "boolean",
    },
    TypedCommandMetadata {
        name: "download_media",
        args_type: "{ request: DownloadMediaRequest }",
        result_type: "void",
    },
    TypedCommandMetadata {
        name: "cancel_media_download",
        args_type: "{ request: CancelMediaDownloadRequest }",
        result_type: "boolean",
    },
    TypedCommandMetadata {
        name: "cancel_timeline_query",
        args_type: "{ request: CancelTimelineQueryRequest }",
        result_type: "boolean",
    },
    TypedCommandMetadata {
        name: "cancel_quote_consumer",
        args_type: "{ request: CancelQuoteConsumerRequest }",
        result_type: "boolean",
    },
    TypedCommandMetadata {
        name: "cancel_mutation_operation",
        args_type: "{ request: CancelMutationOperationRequest }",
        result_type: "boolean",
    },
    TypedCommandMetadata {
        name: "load_timeline",
        args_type: "{ request: TimelineRequest }",
        result_type: "TimelineStatus[]",
    },
    TypedCommandMetadata {
        name: "load_more_timeline",
        args_type: "{ request: TimelineRequest }",
        result_type: "TimelinePageResponse",
    },
    TypedCommandMetadata {
        name: "load_timeline_gap",
        args_type: "{ request: TimelineRequest }",
        result_type: "TimelinePageResponse",
    },
    TypedCommandMetadata {
        name: "refresh_timeline",
        args_type: "{ request: TimelineRequest }",
        result_type: "TimelinePageResponse",
    },
    TypedCommandMetadata {
        name: "account_lists",
        args_type: "{ request: AccountListsRequest }",
        result_type: "AccountListSummary[]",
    },
    TypedCommandMetadata {
        name: "account_feeds",
        args_type: "{ request: AccountFeedsRequest }",
        result_type: "AccountFeedSummary[]",
    },
    TypedCommandMetadata {
        name: "account_profile",
        args_type: "{ request: AccountProfileRequest }",
        result_type: "AccountProfileSummary",
    },
    TypedCommandMetadata {
        name: "account_timeline",
        args_type: "{ request: AccountTimelineRequest }",
        result_type: "AccountTimelinePageResponse",
    },
    TypedCommandMetadata {
        name: "account_follow_action",
        args_type: "{ request: AccountFollowRequest }",
        result_type: "AccountRelationshipSummary",
    },
    TypedCommandMetadata {
        name: "set_account_notification_mute",
        args_type: "{ request: AccountNotificationMuteRequest }",
        result_type: "boolean",
    },
    TypedCommandMetadata {
        name: "switch_active_account",
        args_type: "{ acct: string }",
        result_type: "AppSnapshot",
    },
    TypedCommandMetadata {
        name: "logout_account",
        args_type: "{ acct: string }",
        result_type: "AppSnapshot",
    },
    TypedCommandMetadata {
        name: "status_action",
        args_type: "{ request: StatusActionRequest }",
        result_type: "TimelineStatus",
    },
    TypedCommandMetadata {
        name: "vote_poll",
        args_type: "{ request: VotePollRequest }",
        result_type: "PollSummary",
    },
    TypedCommandMetadata {
        name: "edit_own_status",
        args_type: "{ request: EditStatusRequest }",
        result_type: "TimelineStatus",
    },
    TypedCommandMetadata {
        name: "delete_own_status",
        args_type: "{ request: DeleteStatusRequest }",
        result_type: "void",
    },
    TypedCommandMetadata {
        name: "post_status",
        args_type: "{ request: PostRequest }",
        result_type: "TimelineStatus",
    },
    TypedCommandMetadata {
        name: "enqueue_post_status",
        args_type: "{ request: PostRequest }",
        result_type: "ComposeOutboxItem",
    },
    TypedCommandMetadata {
        name: "enqueue_edit_status",
        args_type: "{ request: EditStatusRequest }",
        result_type: "ComposeOutboxItem",
    },
    TypedCommandMetadata {
        name: "compose_outbox_items",
        args_type: "undefined",
        result_type: "ComposeOutboxItem[]",
    },
    TypedCommandMetadata {
        name: "retry_compose_outbox_item",
        args_type: "{ request: ComposeOutboxItemRequest }",
        result_type: "ComposeOutboxItem",
    },
    TypedCommandMetadata {
        name: "cancel_compose_outbox_item",
        args_type: "{ request: ComposeOutboxItemRequest }",
        result_type: "ComposeOutboxItem",
    },
    TypedCommandMetadata {
        name: "begin_compose_media_upload",
        args_type: "{ request: BeginMediaUploadRequest }",
        result_type: "{ uploadId: string }",
    },
    TypedCommandMetadata {
        name: "finish_compose_media_upload",
        args_type: "{ request: MediaUploadIdRequest }",
        result_type: "MediaAttachment",
    },
    TypedCommandMetadata {
        name: "cancel_compose_media_upload",
        args_type: "{ request: MediaUploadIdRequest }",
        result_type: "void",
    },
    TypedCommandMetadata {
        name: "claim_dropped_media_path",
        args_type: "{ request: ClaimDroppedMediaPathRequest }",
        result_type: "{ capability: string }",
    },
    TypedCommandMetadata {
        name: "upload_compose_media_path",
        args_type: "{ request: UploadMediaPathRequest }",
        result_type: "MediaAttachment",
    },
    TypedCommandMetadata {
        name: "autocomplete_mentions",
        args_type: "{ request: ComposeSuggestionRequest }",
        result_type: "MentionSuggestion[]",
    },
    TypedCommandMetadata {
        name: "autocomplete_hashtags",
        args_type: "{ request: ComposeSuggestionRequest }",
        result_type: "HashtagSuggestion[]",
    },
    TypedCommandMetadata {
        name: "custom_emojis",
        args_type: "{ accountAcct: string }",
        result_type: "CustomEmojiSummary[]",
    },
    TypedCommandMetadata {
        name: "save_settings",
        args_type: "{ request: SaveSettingsRequest }",
        result_type: "SettingsSnapshot",
    },
    TypedCommandMetadata {
        name: "plugin_snapshot",
        args_type: "undefined",
        result_type: "PluginSnapshot",
    },
    TypedCommandMetadata {
        name: "open_plugin_directory",
        args_type: "undefined",
        result_type: "void",
    },
    TypedCommandMetadata {
        name: "reload_plugins",
        args_type: "undefined",
        result_type: "PluginSnapshot",
    },
    TypedCommandMetadata {
        name: "reload_plugin",
        args_type: "{ request: PluginIdRequest }",
        result_type: "PluginSnapshot",
    },
    TypedCommandMetadata {
        name: "unload_plugin",
        args_type: "{ request: PluginIdRequest }",
        result_type: "PluginSnapshot",
    },
    TypedCommandMetadata {
        name: "invoke_plugin_compose_button",
        args_type: "{ request: PluginComposeButtonRequest }",
        result_type: "unknown",
    },
    TypedCommandMetadata {
        name: "translate_status_text",
        args_type: "{ request: TranslateStatusRequest }",
        result_type: "{ text: string; sourceLanguage?: string | null; targetLanguage: string }",
    },
    TypedCommandMetadata {
        name: "vacuum_database",
        args_type: "undefined",
        result_type: "DbSummary",
    },
    TypedCommandMetadata {
        name: "clear_status_cache",
        args_type: "undefined",
        result_type: "DbSummary",
    },
    TypedCommandMetadata {
        name: "save_columns",
        args_type: "{ request: SaveColumnsRequest }",
        result_type: "AppSnapshot",
    },
    TypedCommandMetadata {
        name: "create_sidecar_webview",
        args_type: "{ request: CreateSidecarWebviewRequest }",
        result_type: "void",
    },
    TypedCommandMetadata {
        name: "navigate_sidecar_webview",
        args_type: "{ sidecarId: string; url: string }",
        result_type: "void",
    },
    TypedCommandMetadata {
        name: "reload_sidecar_webview",
        args_type: "{ sidecarId: string }",
        result_type: "void",
    },
    TypedCommandMetadata {
        name: "close_sidecar_webview",
        args_type: "{ sidecarId: string }",
        result_type: "void",
    },
    TypedCommandMetadata {
        name: "scroll_sidecar_webview_to_top",
        args_type: "{ sidecarId: string }",
        result_type: "void",
    },
    TypedCommandMetadata {
        name: "inject_sidecar_user_style",
        args_type: "{ sidecarId: string; userStyle: string }",
        result_type: "void",
    },
    TypedCommandMetadata {
        name: "start_runtime_initialization",
        args_type: "undefined",
        result_type: "void",
    },
    TypedCommandMetadata {
        name: "retry_runtime_initialization",
        args_type: "undefined",
        result_type: "void",
    },
    TypedCommandMetadata {
        name: "app_snapshot",
        args_type: "undefined",
        result_type: "AppSnapshot",
    },
    TypedCommandMetadata {
        name: "report_release_webview_smoke",
        args_type: "{ report: ReleaseWebviewSmokeReport }",
        result_type: "void",
    },
    TypedCommandMetadata {
        name: "account_summaries",
        args_type: "undefined",
        result_type: "AccountSummary[]",
    },
    TypedCommandMetadata {
        name: "get_web_socket_statuses",
        args_type: "undefined",
        result_type: "WebSocketStatus[]",
    },
    TypedCommandMetadata {
        name: "reconnect_web_socket",
        args_type: "{ id?: string | null }",
        result_type: "void",
    },
    TypedCommandMetadata {
        name: "status_bar_snapshot",
        args_type: "undefined",
        result_type: "Omit<StatusBarSnapshot, \"fetchedAt\">",
    },
    TypedCommandMetadata {
        name: "notification_muted_accounts",
        args_type: "undefined",
        result_type: "NotificationMutedAccountSummary[]",
    },
    TypedCommandMetadata {
        name: "status_viewer_states",
        args_type: "{ request: StatusViewerStatesRequest }",
        result_type: "StatusViewerStateSummary[]",
    },
    TypedCommandMetadata {
        name: "status_thread",
        args_type: "{ request: StatusThreadRequest }",
        result_type: "TimelineStatus[]",
    },
    TypedCommandMetadata {
        name: "air_context",
        args_type: "{ request: AirContextRequest }",
        result_type: "TimelineStatus[]",
    },
    TypedCommandMetadata {
        name: "support_bundle",
        args_type: "{ request: { operationId?: string | null; frontend: FrontendHealthSnapshot } }",
        result_type: "SupportBundle",
    },
    TypedCommandMetadata {
        name: "diagnostics_snapshot",
        args_type: "undefined",
        result_type: "DiagnosticsSnapshot",
    },
    TypedCommandMetadata {
        name: "explain_custom_timeline",
        args_type: "{ request: ExplainCustomTimelineRequest }",
        result_type: "{ id: number; parent: number; detail: string }[]",
    },
    TypedCommandMetadata {
        name: "icu_match_expression",
        args_type: "{ request: IcuMatchExpressionRequest }",
        result_type: "string | null",
    },
    TypedCommandMetadata {
        name: "open_status_url",
        args_type: "{ url: string }",
        result_type: "void",
    },
    TypedCommandMetadata {
        name: "open_log_file",
        args_type: "undefined",
        result_type: "void",
    },
];

#[allow(dead_code)] // Consumed by the standalone contract generator target.
pub const RAW_COMMANDS: &[RawCommandMetadata] = &[RawCommandMetadata {
    name: "append_compose_media_upload",
    result_type: "{ written: number; total: number }",
}];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dto_metadata_matches_serde_field_names_and_optionality() {
        let download = DTOS
            .iter()
            .find(|dto| dto.name == "DownloadMediaRequest")
            .expect("download DTO");
        assert_eq!(download.fields[0].serialized_name, "operationId");
        assert!(download.fields[0].optional);
        assert_eq!(download.fields[1].rust_name, "url");
        assert!(!download.fields[1].optional);
    }

    #[test]
    fn plugin_compose_button_request_uses_camel_case_and_preserves_json() {
        let request: PluginComposeButtonRequest = serde_json::from_value(serde_json::json!({
            "pluginId": "sample",
            "buttonId": "compose-0",
            "generation": 7,
            "compose": { "text": "hello", "cw_title": "notice" }
        }))
        .expect("plugin compose request");

        assert_eq!(request.plugin_id, "sample");
        assert_eq!(request.button_id, "compose-0");
        assert_eq!(request.generation, 7);
        assert_eq!(request.compose["cw_title"], "notice");
    }
}
