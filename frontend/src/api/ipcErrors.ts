import { t, type MessageId } from "../i18n";

export const APP_ERROR_CODES = [
  "authentication_expired",
  "rate_limited",
  "timeout",
  "validation",
  "database_busy",
  "capability_unsupported",
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
  ipc_response_lost: "errors.ipc_response_lost",
  internal: "errors.internal",
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
    super(t(SAFE_MESSAGES[envelope.code]));
    this.name = "IpcAppError";
    this.code = envelope.code;
    this.messageKey = envelope.messageKey;
    this.safeDetails = envelope.safeDetails;
    this.retryable = envelope.retryable;
    this.requestId = envelope.requestId;
  }
}

export function normalizeIpcError(
  error: unknown,
  requestId: string,
): IpcAppError {
  if (error instanceof IpcAppError) return error;
  if (isAppErrorEnvelope(error)) {
    return new IpcAppError({
      code: error.code,
      messageKey: isSafeMessageKey(error.messageKey)
        ? error.messageKey
        : DEFAULT_MESSAGE_KEYS[error.code],
      safeDetails: sanitizeSafeDetails(error.safeDetails),
      retryable: RETRYABLE_CODES.has(error.code) && error.retryable,
      requestId: isUuid(error.requestId) ? error.requestId : requestId,
    });
  }

  const transportCode = transportErrorCode(error);
  const code = transportCode ?? "internal";
  return new IpcAppError({
    code,
    messageKey: DEFAULT_MESSAGE_KEYS[code],
    retryable: RETRYABLE_CODES.has(code),
    requestId,
  });
}

export function isResponseLossError(error: unknown) {
  return error instanceof IpcAppError && error.code === "ipc_response_lost";
}

/** Convert any failure into reviewed text that is safe to render in the UI. */
export function publicErrorMessage(error: unknown) {
  // The fallback request ID is not rendered. It only gives unclassified
  // browser/library failures the same safe envelope as an IPC failure, so a
  // raw local path, URL query, SQL fragment, or credential never reaches a
  // toast through `String(error)`.
  return normalizeIpcError(
    error,
    "00000000-0000-4000-8000-000000000000",
  ).message;
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
  const allowedKeys = new Set(["retryAfterSeconds", "field", "limit"]);
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

function isSafeMessageKey(value: string) {
  return /^errors\.[a-z_]+$/.test(value);
}

function isUuid(value: string) {
  return /^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i.test(
    value,
  );
}
