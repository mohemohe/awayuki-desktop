import { invoke as tauriInvoke } from "@tauri-apps/api/core";

import type {
  IpcCommandName,
  RawIpcCommand,
  RawIpcCommandResult,
  RetryableReadCommand,
  TypedIpcCommand,
  TypedIpcCommandArgs,
  TypedIpcCommandResult,
} from "./generated/contract";
import {
  IpcAppError,
  isResponseLossError,
  normalizeIpcError,
} from "./ipcErrors";
import {
  completeUiOperation,
  startUiOperation,
} from "./observability";

const TRANSIENT_READ_RETRY_DELAY_MS = 75;

export type { RetryableReadCommand } from "./generated/contract";

type InvokePolicy = {
  transientRetries: 0 | 1;
};

const NO_RETRY: InvokePolicy = { transientRetries: 0 };
const RETRY_TRANSIENT_READ_ONCE: InvokePolicy = { transientRetries: 1 };

export function hasTauriRuntime() {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

export function invokeTypedCommand<C extends TypedIpcCommand>(
  command: C,
  ...args: TypedIpcCommandArgs[C] extends undefined
    ? []
    : [args: TypedIpcCommandArgs[C]]
): Promise<TypedIpcCommandResult[C]> {
  return invokeWithPolicy<TypedIpcCommandResult[C]>(
    command,
    args[0] as Record<string, unknown> | undefined,
    NO_RETRY,
  );
}

export function invokeTypedCommandWithOperationId<C extends TypedIpcCommand>(
  command: C,
  args: Exclude<TypedIpcCommandArgs[C], undefined>,
  operationId: string,
): Promise<TypedIpcCommandResult[C]> {
  return invokeWithPolicy<TypedIpcCommandResult[C]>(
    command,
    args as Record<string, unknown>,
    NO_RETRY,
    operationId,
  );
}

export async function invokeRawCommand<C extends RawIpcCommand>(
  command: C,
  body: Uint8Array,
  headers: HeadersInit = {},
): Promise<RawIpcCommandResult[C]> {
  const startedAt = performance.now();
  const operationId = startUiOperation();
  const requestHeaders = new Headers(headers);
  requestHeaders.set("x-awayuki-operation-id", operationId);
  console.debug(
    `[awayuki][ui-ipc] start operation_id=${operationId} command=${command} raw_bytes=${body.byteLength}`,
  );
  try {
    const result = hasTauriRuntime()
      ? await tauriInvoke<RawIpcCommandResult[C]>(command, body, { headers: requestHeaders })
      : import.meta.env.DEV
        ? await (await import("./mock")).mockInvokeRaw<RawIpcCommandResult[C]>(
            command,
            body,
            requestHeaders,
          )
        : await Promise.reject(
            new Error("Tauri IPC is unavailable outside the desktop runtime"),
          );
    completeUiOperation(false);
    console.debug(
      `[awayuki][ui-ipc] success operation_id=${operationId} command=${command} duration_ms=${elapsedMs(startedAt)} result=${summarizeInvokeResult(result)}`,
    );
    return result;
  } catch (rawError) {
    const error = normalizeIpcError(rawError, operationId, command);
    completeUiOperation(true);
    console.error(
      `[awayuki][ui-ipc] error operation_id=${operationId} command=${command} duration_ms=${elapsedMs(startedAt)} ${formatInvokeError(error)}`,
    );
    throw error;
  }
}

export function invokeTypedReadCommand<
  C extends TypedIpcCommand & RetryableReadCommand,
>(
  command: C,
  ...args: TypedIpcCommandArgs[C] extends undefined
    ? []
    : [args: TypedIpcCommandArgs[C]]
): Promise<TypedIpcCommandResult[C]> {
  return invokeWithPolicy<TypedIpcCommandResult[C]>(
    command,
    args[0] as Record<string, unknown> | undefined,
    RETRY_TRANSIENT_READ_ONCE,
  );
}

export function invokeTypedReadCommandWithOperationId<
  C extends TypedIpcCommand & RetryableReadCommand,
>(
  command: C,
  args: Exclude<TypedIpcCommandArgs[C], undefined>,
  operationId: string,
): Promise<TypedIpcCommandResult[C]> {
  return invokeWithPolicy<TypedIpcCommandResult[C]>(
    command,
    args as Record<string, unknown>,
    RETRY_TRANSIENT_READ_ONCE,
    operationId,
  );
}

async function invokeWithPolicy<T>(
  command: IpcCommandName,
  args: Record<string, unknown> | undefined,
  policy: InvokePolicy,
  requestedOperationId?: string,
): Promise<T> {
  const startedAt = performance.now();
  const generatedOperationId = startUiOperation();
  const operationId = isUuid(requestedOperationId)
    ? requestedOperationId
    : generatedOperationId;
  const tracedArgs = withOperationId(args, operationId);
  const invokeOptions = {
    headers: { "x-awayuki-operation-id": operationId },
  };
  const argsSummary = summarizeInvokeArgs(tracedArgs);
  console.debug(
    `[awayuki][ui-ipc] start operation_id=${operationId} command=${command} attempt=1 args=${argsSummary}`,
  );
  if (hasTauriRuntime()) {
    try {
      const result = await tauriInvoke<T>(command, tracedArgs, invokeOptions);
      completeUiOperation(false);
      console.debug(
        `[awayuki][ui-ipc] success operation_id=${operationId} command=${command} attempt=1 duration_ms=${elapsedMs(startedAt)} result=${summarizeInvokeResult(result)}`,
      );
      return result;
    } catch (rawError) {
      const error = normalizeIpcError(rawError, operationId, command);
      if (
        policy.transientRetries === 0 ||
        !isResponseLossError(error)
      ) {
        completeUiOperation(true);
        console.error(
          `[awayuki][ui-ipc] error operation_id=${operationId} command=${command} attempt=1 duration_ms=${elapsedMs(startedAt)} ${formatInvokeError(error)}`,
        );
        throw error;
      }
      console.debug(
        `[awayuki][ui-ipc] retry operation_id=${operationId} command=${command} attempt=2 after_ms=${elapsedMs(startedAt)} ${formatInvokeError(error)}`,
      );
      await delay(TRANSIENT_READ_RETRY_DELAY_MS);
      try {
        const result = await tauriInvoke<T>(command, tracedArgs, invokeOptions);
        completeUiOperation(false);
        console.debug(
          `[awayuki][ui-ipc] success operation_id=${operationId} command=${command} attempt=2 duration_ms=${elapsedMs(startedAt)} result=${summarizeInvokeResult(result)}`,
        );
        return result;
      } catch (rawRetryError) {
        const retryError = normalizeIpcError(rawRetryError, operationId, command);
        completeUiOperation(true);
        console.error(
          `[awayuki][ui-ipc] error operation_id=${operationId} command=${command} attempt=2 duration_ms=${elapsedMs(startedAt)} ${formatInvokeError(retryError)}`,
        );
        throw retryError;
      }
    }
  }
  try {
    if (import.meta.env.DEV) {
      const { mockInvoke } = await import("./mock");
      const result = await mockInvoke<T>(command, tracedArgs);
      completeUiOperation(false);
      console.debug(
        `[awayuki][ui-ipc] success operation_id=${operationId} command=${command} runtime=mock attempt=1 duration_ms=${elapsedMs(startedAt)} result=${summarizeInvokeResult(result)}`,
      );
      return result;
    }
    throw new Error("Tauri IPC is unavailable outside the desktop runtime");
  } catch (rawError) {
    const error = normalizeIpcError(rawError, operationId, command);
    completeUiOperation(true);
    console.error(
      `[awayuki][ui-ipc] error operation_id=${operationId} command=${command} runtime=mock attempt=1 duration_ms=${elapsedMs(startedAt)} ${formatInvokeError(error)}`,
    );
    throw error;
  }
}

function isUuid(value: unknown): value is string {
  return Boolean(
    typeof value === "string" &&
      /^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i.test(
        value,
      ),
  );
}

function withOperationId(
  args: Record<string, unknown> | undefined,
  operationId: string,
) {
  if (!args) return args;
  const request = args.request;
  if (!request || typeof request !== "object" || Array.isArray(request)) {
    return args;
  }
  return {
    ...args,
    request: {
      ...(request as Record<string, unknown>),
      operationId,
    },
  };
}

function delay(ms: number) {
  return new Promise<void>((resolve) => {
    window.setTimeout(resolve, ms);
  });
}

function elapsedMs(startedAt: number) {
  return (performance.now() - startedAt).toFixed(1);
}

function summarizeInvokeArgs(args?: Record<string, unknown>) {
  if (!args) return "{}";
  const request =
    args.request &&
    typeof args.request === "object" &&
    !Array.isArray(args.request)
      ? Object.keys(args.request as Record<string, unknown>).sort()
      : [];
  return JSON.stringify({ keys: Object.keys(args).sort(), requestKeys: request });
}

function summarizeInvokeResult(result: unknown) {
  if (Array.isArray(result)) return `array:${result.length}`;
  if (result == null) return String(result);
  if (typeof result !== "object") return typeof result;
  return `object:${Object.keys(result as Record<string, unknown>).join(",")}`;
}

function formatInvokeError(error: IpcAppError) {
  return `error_code=${error.code} request_id=${error.requestId}`;
}
