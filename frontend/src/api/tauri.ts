import { invoke as tauriInvoke } from "@tauri-apps/api/core";
import { mockInvoke } from "./mock";

export function hasTauriRuntime() {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

export async function invokeCommand<T = unknown>(
  command: string,
  args?: Record<string, unknown>,
): Promise<T> {
  const startedAt = performance.now();
  const argsSummary = summarizeInvokeArgs(args);
  console.debug(
    `[awayuki][ui-ipc] start command=${command} args=${argsSummary}`,
  );
  if (hasTauriRuntime()) {
    try {
      const result = await tauriInvoke<T>(command, args);
      console.debug(
        `[awayuki][ui-ipc] success command=${command} duration_ms=${elapsedMs(startedAt)} result=${summarizeInvokeResult(result)}`,
      );
      return result;
    } catch (error) {
      if (!isTransientTauriIpcError(error)) {
        console.debug(
          `[awayuki][ui-ipc] error command=${command} duration_ms=${elapsedMs(startedAt)} error=${formatInvokeError(error)}`,
        );
        throw error;
      }
      console.debug(
        `[awayuki][ui-ipc] retry command=${command} after_ms=${elapsedMs(startedAt)} error=${formatInvokeError(error)}`,
      );
      await delay(75);
      try {
        const result = await tauriInvoke<T>(command, args);
        console.debug(
          `[awayuki][ui-ipc] success command=${command} attempt=retry duration_ms=${elapsedMs(startedAt)} result=${summarizeInvokeResult(result)}`,
        );
        return result;
      } catch (retryError) {
        console.debug(
          `[awayuki][ui-ipc] error command=${command} attempt=retry duration_ms=${elapsedMs(startedAt)} error=${formatInvokeError(retryError)}`,
        );
        throw retryError;
      }
    }
  }
  try {
    const result = await mockInvoke<T>(command, args);
    console.debug(
      `[awayuki][ui-ipc] success command=${command} runtime=mock duration_ms=${elapsedMs(startedAt)} result=${summarizeInvokeResult(result)}`,
    );
    return result;
  } catch (error) {
    console.debug(
      `[awayuki][ui-ipc] error command=${command} runtime=mock duration_ms=${elapsedMs(startedAt)} error=${formatInvokeError(error)}`,
    );
    throw error;
  }
}

function isTransientTauriIpcError(error: unknown) {
  const message =
    error instanceof Error
      ? error.message
      : typeof error === "string"
        ? error
        : "";
  return (
    message.includes("Load failed") ||
    message.includes("IPC custom protocol failed") ||
    message.includes("postMessage interface")
  );
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
  try {
    return JSON.stringify(summarizeLogValue(args, 0));
  } catch {
    return '"[unserializable]"';
  }
}

function summarizeInvokeResult(result: unknown) {
  if (Array.isArray(result)) return `array:${result.length}`;
  if (result == null) return String(result);
  if (typeof result !== "object") return typeof result;
  return `object:${Object.keys(result as Record<string, unknown>).join(",")}`;
}

function summarizeLogValue(value: unknown, depth: number): unknown {
  if (value == null) return value;
  if (typeof value === "string") return summarizeString(value);
  if (typeof value === "number" || typeof value === "boolean") return value;
  if (Array.isArray(value)) return `[array:${value.length}]`;
  if (typeof value !== "object") return typeof value;
  if (depth >= 3) return "{...}";

  return Object.fromEntries(
    Object.entries(value as Record<string, unknown>).map(([key, entry]) => [
      key,
      shouldRedactLogField(key)
        ? "[redacted]"
        : summarizeLogValue(entry, depth + 1),
    ]),
  );
}

function summarizeString(value: string) {
  if (value.length <= 160) return value;
  return `${value.slice(0, 120)}...[len=${value.length}]`;
}

function shouldRedactLogField(key: string) {
  const normalized = key.toLowerCase();
  return (
    normalized.includes("password") ||
    normalized.includes("token") ||
    normalized.includes("secret") ||
    normalized.includes("credential") ||
    normalized === "content" ||
    normalized === "text" ||
    normalized.endsWith("html") ||
    normalized.includes("file") ||
    normalized.includes("path")
  );
}

function formatInvokeError(error: unknown) {
  if (error instanceof Error) return summarizeString(error.message);
  if (typeof error === "string") return summarizeString(error);
  try {
    return JSON.stringify(error);
  } catch {
    return String(error);
  }
}
