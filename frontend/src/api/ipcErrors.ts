import { t, type MessageId } from "../i18n";
import type { IpcCommandName } from "./generated/contract";

export const APP_ERROR_CODES = [
  "authentication_expired",
  "rate_limited",
  "timeout",
  "validation",
  "database_busy",
  "capability_unsupported",
  "cancelled",
  "ipc_response_lost",
  "internal",
] as const;

export type AppErrorCode = (typeof APP_ERROR_CODES)[number];

export type AppErrorEnvelope = {
  code: AppErrorCode;
  messageKey: string;
  safeDetails?: Record<string, string>;
  retryable: boolean;
  requestId: string;
};

const SAFE_MESSAGES: Record<AppErrorCode, MessageId> = {
  authentication_expired: "Authentication expired. Please sign in again.",
  rate_limited: "The server rate limit was reached. Please try again later.",
  timeout: "The operation timed out. Please try again.",
  validation: "The request is invalid. Please review the input.",
  database_busy: "The database is busy. Please try again.",
  capability_unsupported: "This operation is not supported by the account.",
  cancelled: "The operation was cancelled.",
  ipc_response_lost:
    "The backend response was lost. Refresh before retrying a change.",
  internal: "The operation failed. Please try again.",
};

const DEFAULT_MESSAGE_KEYS: Record<AppErrorCode, string> = {
  authentication_expired: "errors.authentication_expired",
  rate_limited: "errors.rate_limited",
  timeout: "errors.timeout",
  validation: "errors.validation",
  database_busy: "errors.database_busy",
  capability_unsupported: "errors.capability_unsupported",
  cancelled: "errors.cancelled",
  ipc_response_lost: "errors.ipc_response_lost",
  internal: "errors.internal",
};

const REVIEWED_MESSAGE_KEYS = {
  "errors.authentication_expired":
    "Authentication expired. Please sign in again.",
  "errors.rate_limited":
    "The server rate limit was reached. Please try again later.",
  "errors.timeout": "The operation timed out. Please try again.",
  "errors.validation": "The request is invalid. Please review the input.",
  "errors.database_busy": "The database is busy. Please try again.",
  "errors.capability_unsupported":
    "This operation is not supported by the account.",
  "errors.cancelled": "The operation was cancelled.",
  "errors.ipc_response_lost":
    "The backend response was lost. Refresh before retrying a change.",
  "errors.internal": "The operation failed. Please try again.",
  "errors.startup_failed":
    "Awayuki could not restore its local data. Please try again.",
  "errors.account_load_failed":
    "The account information could not be loaded. Please try again.",
  "errors.login_failed":
    "Sign-in failed. Please review the account information and try again.",
  "errors.timeline_load_failed":
    "The timeline could not be loaded. Please try again.",
  "errors.account_action_failed":
    "The account operation failed. Please try again.",
  "errors.notification_setting_failed":
    "The notification setting could not be changed. Please try again.",
  "errors.post_failed":
    "The post could not be saved. Please review the content and try again.",
  "errors.media_failed": "The media operation failed. Please try again.",
  "errors.suggestion_load_failed":
    "Suggestions could not be loaded. Please try again.",
  "errors.settings_save_failed":
    "The settings could not be saved. Please try again.",
  "errors.translation_failed": "Translation failed. Please try again.",
  "errors.custom_timeline_failed":
    "Custom timeline SQL could not be executed. Review the query and try again.",
  "errors.custom_timeline_fts_match_or":
    "FTS search conditions are invalid. Combine alternatives inside one MATCH expression with OR.",
  "errors.kq_invalid_query":
    "KQ query is invalid. Review the query and try again.",
  "errors.kq_query_budget_exceeded":
    "KQ query exceeded its evaluation limit. Narrow the query and try again.",
  "errors.database_operation_failed":
    "The database operation failed. Please try again.",
  "errors.status_action_failed":
    "The post operation failed. Please try again.",
  "errors.media_download_failed":
    "The media could not be saved. Please try again.",
  "errors.external_open_failed":
    "The requested item could not be opened. Please try again.",
  "errors.sidecar_failed":
    "The sidecar operation failed. Please try again.",
  "errors.diagnostics_failed":
    "Diagnostics could not be created. Please try again.",
} as const satisfies Record<string, MessageId>;

type ReviewedMessageKey = keyof typeof REVIEWED_MESSAGE_KEYS;

const COMMAND_MESSAGE_KEYS: Partial<Record<IpcCommandName, ReviewedMessageKey>> = {
  app_snapshot: "errors.startup_failed",
  start_runtime_initialization: "errors.startup_failed",
  retry_runtime_initialization: "errors.startup_failed",
  account_summaries: "errors.account_load_failed",
  account_lists: "errors.account_load_failed",
  login_with_instance_domain: "errors.login_failed",
  login_with_bluesky_app_password: "errors.login_failed",
  load_timeline: "errors.timeline_load_failed",
  load_more_timeline: "errors.timeline_load_failed",
  refresh_timeline: "errors.timeline_load_failed",
  status_viewer_states: "errors.timeline_load_failed",
  status_thread: "errors.timeline_load_failed",
  air_context: "errors.timeline_load_failed",
  account_profile: "errors.account_load_failed",
  account_timeline: "errors.account_load_failed",
  account_follow_action: "errors.account_action_failed",
  notification_muted_accounts: "errors.account_load_failed",
  set_account_notification_mute: "errors.notification_setting_failed",
  post_status: "errors.post_failed",
  enqueue_post_status: "errors.post_failed",
  enqueue_edit_status: "errors.post_failed",
  compose_outbox_items: "errors.post_failed",
  retry_compose_outbox_item: "errors.post_failed",
  cancel_compose_outbox_item: "errors.post_failed",
  begin_compose_media_upload: "errors.media_failed",
  append_compose_media_upload: "errors.media_failed",
  finish_compose_media_upload: "errors.media_failed",
  cancel_compose_media_upload: "errors.media_failed",
  claim_dropped_media_path: "errors.media_failed",
  upload_compose_media_path: "errors.media_failed",
  autocomplete_mentions: "errors.suggestion_load_failed",
  autocomplete_hashtags: "errors.suggestion_load_failed",
  custom_emojis: "errors.suggestion_load_failed",
  edit_own_status: "errors.post_failed",
  delete_own_status: "errors.post_failed",
  vote_poll: "errors.status_action_failed",
  switch_active_account: "errors.account_action_failed",
  logout_account: "errors.account_action_failed",
  save_settings: "errors.settings_save_failed",
  translate_status_text: "errors.translation_failed",
  save_columns: "errors.settings_save_failed",
  explain_custom_timeline: "errors.custom_timeline_failed",
  vacuum_database: "errors.database_operation_failed",
  clear_status_cache: "errors.database_operation_failed",
  status_bar_snapshot: "errors.diagnostics_failed",
  status_action: "errors.status_action_failed",
  download_media: "errors.media_download_failed",
  open_status_url: "errors.external_open_failed",
  create_sidecar_webview: "errors.sidecar_failed",
  navigate_sidecar_webview: "errors.sidecar_failed",
  reload_sidecar_webview: "errors.sidecar_failed",
  close_sidecar_webview: "errors.sidecar_failed",
  scroll_sidecar_webview_to_top: "errors.sidecar_failed",
  inject_sidecar_user_style: "errors.sidecar_failed",
  open_log_file: "errors.external_open_failed",
  diagnostics_snapshot: "errors.diagnostics_failed",
  support_bundle: "errors.diagnostics_failed",
};

const RETRYABLE_CODES = new Set<AppErrorCode>([
  "rate_limited",
  "timeout",
  "database_busy",
  "ipc_response_lost",
]);

const KNOWN_TRANSPORT_MESSAGES = new Set([
  "Load failed",
  "IPC custom protocol failed",
  "postMessage interface is not available",
]);

export class IpcAppError extends Error implements AppErrorEnvelope {
  readonly code: AppErrorCode;
  readonly messageKey: string;
  readonly safeDetails?: Record<string, string>;
  readonly retryable: boolean;
  readonly requestId: string;

  constructor(envelope: AppErrorEnvelope) {
    super(t(reviewedMessage(envelope.messageKey, envelope.code)));
    this.name = "IpcAppError";
    this.code = envelope.code;
    this.messageKey = envelope.messageKey;
    this.safeDetails = envelope.safeDetails;
    this.retryable = envelope.retryable;
    this.requestId = envelope.requestId;
  }

  override toString() {
    return this.message;
  }
}

export function normalizeIpcError(
  error: unknown,
  requestId: string,
  command?: IpcCommandName,
): IpcAppError {
  if (error instanceof IpcAppError) return error;
  if (isAppErrorEnvelope(error)) {
    const messageKey = normalizedMessageKey(error.messageKey, error.code, command);
    return new IpcAppError({
      code: error.code,
      messageKey,
      safeDetails: sanitizeSafeDetails(error.safeDetails),
      retryable: RETRYABLE_CODES.has(error.code) && error.retryable,
      requestId: isUuid(error.requestId) ? error.requestId : requestId,
    });
  }

  const transportCode = transportErrorCode(error);
  const code = transportCode ?? "internal";
  return new IpcAppError({
    code,
    messageKey: contextualMessageKey(DEFAULT_MESSAGE_KEYS[code], code, command),
    retryable: RETRYABLE_CODES.has(code),
    requestId,
  });
}

export function isResponseLossError(error: unknown) {
  return error instanceof IpcAppError && error.code === "ipc_response_lost";
}

/** Convert any failure into reviewed text that is safe to render in the UI. */
export function publicErrorMessage(error: unknown, command?: IpcCommandName) {
  // The fallback request ID is not rendered. It only gives unclassified
  // browser/library failures the same safe envelope as an IPC failure, so a
  // raw local path, URL query, SQL fragment, or credential never reaches a
  // toast through `String(error)`.
  return normalizeIpcError(
    error,
    "00000000-0000-4000-8000-000000000000",
    command,
  ).message;
}

function normalizedMessageKey(
  messageKey: string,
  code: AppErrorCode,
  command?: IpcCommandName,
) {
  const reviewed = isReviewedMessageKey(messageKey)
    ? messageKey
    : DEFAULT_MESSAGE_KEYS[code];
  return contextualMessageKey(reviewed, code, command);
}

function contextualMessageKey(
  messageKey: string,
  code: AppErrorCode,
  command?: IpcCommandName,
) {
  if (messageKey !== DEFAULT_MESSAGE_KEYS.internal || code !== "internal") {
    return messageKey;
  }
  return (command && COMMAND_MESSAGE_KEYS[command]) || messageKey;
}

function reviewedMessage(messageKey: string, code: AppErrorCode): MessageId {
  return isReviewedMessageKey(messageKey)
    ? REVIEWED_MESSAGE_KEYS[messageKey]
    : SAFE_MESSAGES[code];
}

function isAppErrorEnvelope(error: unknown): error is AppErrorEnvelope {
  if (!error || typeof error !== "object") return false;
  const candidate = error as Partial<AppErrorEnvelope>;
  return (
    typeof candidate.code === "string" &&
    APP_ERROR_CODES.includes(candidate.code as AppErrorCode) &&
    typeof candidate.messageKey === "string" &&
    typeof candidate.retryable === "boolean" &&
    typeof candidate.requestId === "string"
  );
}

function transportErrorCode(error: unknown): AppErrorCode | undefined {
  if (error && typeof error === "object") {
    const code = (error as { code?: unknown }).code;
    if (
      code === "IPC_RESPONSE_LOST" ||
      code === "ERR_IPC_CHANNEL_CLOSED" ||
      code === "ERR_FAILED"
    ) {
      return "ipc_response_lost";
    }
  }
  const message =
    error instanceof Error
      ? error.message
      : typeof error === "string"
        ? error
        : undefined;
  return message && KNOWN_TRANSPORT_MESSAGES.has(message)
    ? "ipc_response_lost"
    : undefined;
}

function sanitizeSafeDetails(value: unknown) {
  if (!value || typeof value !== "object") return undefined;
  const allowedKeys = new Set([
    "retryAfterSeconds",
    "field",
    "limit",
    "line",
    "column",
  ]);
  const entries = Object.entries(value as Record<string, unknown>)
    .filter(
      ([key, entry]) =>
        allowedKeys.has(key) &&
        typeof entry === "string" &&
        entry.length <= 80,
    )
    .map(([key, entry]) => [key, entry as string]);
  return entries.length > 0 ? Object.fromEntries(entries) : undefined;
}

function isReviewedMessageKey(value: string): value is ReviewedMessageKey {
  return Object.prototype.hasOwnProperty.call(REVIEWED_MESSAGE_KEYS, value);
}

function isUuid(value: string) {
  return /^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i.test(
    value,
  );
}
