import { invoke as tauriInvoke } from "@tauri-apps/api/core";

import type {
  IpcCommandName,
  RetryableReadCommand,
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

export async function invokeCommand<T = unknown>(
  command: IpcCommandName,
  args?: Record<string, unknown>,
): Promise<T> {
  return invokeWithPolicy<T>(command, args, NO_RETRY);
}

export async function invokeReadCommand<T = unknown>(
  command: RetryableReadCommand,
  args?: Record<string, unknown>,
): Promise<T> {
  return invokeWithPolicy<T>(command, args, RETRY_TRANSIENT_READ_ONCE);
}

async function invokeWithPolicy<T>(
  command: IpcCommandName,
  args: Record<string, unknown> | undefined,
  policy: InvokePolicy,
): Promise<T> {
  const startedAt = performance.now();
  const operationId = startUiOperation();
  const tracedArgs = withOperationId(args, operationId);
  const argsSummary = summarizeInvokeArgs(tracedArgs);
  console.debug(
    `[awayuki][ui-ipc] start operation_id=${operationId} command=${command} attempt=1 args=${argsSummary}`,
  );
  if (hasTauriRuntime()) {
    try {
      const result = await tauriInvoke<T>(command, tracedArgs);
      completeUiOperation(false);
      console.debug(
        `[awayuki][ui-ipc] success operation_id=${operationId} command=${command} attempt=1 duration_ms=${elapsedMs(startedAt)} result=${summarizeInvokeResult(result)}`,
      );
      return result;
    } catch (rawError) {
      const error = normalizeIpcError(rawError, operationId);
      if (
        policy.transientRetries === 0 ||
        !isResponseLossError(error)
      ) {
        completeUiOperation(true);
        console.debug(
          `[awayuki][ui-ipc] error operation_id=${operationId} command=${command} attempt=1 duration_ms=${elapsedMs(startedAt)} ${formatInvokeError(error)}`,
        );
        throw error;
      }
      console.debug(
        `[awayuki][ui-ipc] retry operation_id=${operationId} command=${command} attempt=2 after_ms=${elapsedMs(startedAt)} ${formatInvokeError(error)}`,
      );
      await delay(TRANSIENT_READ_RETRY_DELAY_MS);
      try {
        const result = await tauriInvoke<T>(command, tracedArgs);
        completeUiOperation(false);
        console.debug(
          `[awayuki][ui-ipc] success operation_id=${operationId} command=${command} attempt=2 duration_ms=${elapsedMs(startedAt)} result=${summarizeInvokeResult(result)}`,
        );
        return result;
      } catch (rawRetryError) {
        const retryError = normalizeIpcError(rawRetryError, operationId);
        completeUiOperation(true);
        console.debug(
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
    const error = normalizeIpcError(rawError, operationId);
    completeUiOperation(true);
    console.debug(
      `[awayuki][ui-ipc] error operation_id=${operationId} command=${command} runtime=mock attempt=1 duration_ms=${elapsedMs(startedAt)} ${formatInvokeError(error)}`,
    );
    throw error;
  }
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
