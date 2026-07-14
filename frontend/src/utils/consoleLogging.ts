import { debug, error, info, trace, warn } from "@tauri-apps/plugin-log";

type ConsoleFnName = "log" | "debug" | "info" | "warn" | "error";
type LogForwarder = (message: string) => Promise<void>;

const consoleForwarders: Record<ConsoleFnName, LogForwarder> = {
  log: trace,
  debug,
  info,
  warn,
  error,
};

let installed = false;
const BATCH_DELAY_MS = 100;
const MAX_BATCH_MESSAGES = 50;
const MAX_BATCH_BYTES = 16 * 1024;
const pendingMessages = new Map<ConsoleFnName, string[]>();
let batchTimer: number | undefined;

function isTauriRuntime(): boolean {
  return "__TAURI_INTERNALS__" in window;
}

function formatConsoleValue(value: unknown): string {
  if (value instanceof Error) {
    return value.stack ?? value.message;
  }
  if (typeof value === "string") {
    return value;
  }
  if (
    typeof value === "number" ||
    typeof value === "boolean" ||
    typeof value === "bigint" ||
    typeof value === "symbol" ||
    value == null
  ) {
    return String(value);
  }
  try {
    return JSON.stringify(value);
  } catch {
    return String(value);
  }
}

function formatConsoleArgs(args: unknown[]): string {
  return redactConsoleMessage(args.map(formatConsoleValue).join(" "));
}

export function redactConsoleMessage(message: string): string {
  return message
    .replace(/\bBearer\s+[A-Za-z0-9._~+/=-]+/gi, "Bearer [redacted]")
    .replace(
      /\b(access[_-]?token|refresh[_-]?token|password|client[_-]?secret|credential|code|state)(\s*[:=]\s*)([^\s&,"'}]+)/gi,
      "$1$2[redacted]",
    )
    .replace(
      /\b(content|spoiler[_-]?text|notification[_-]?body|post[_-]?body|status[_-]?text)(\s*[:=]\s*)(?:"(?:\\.|[^"])*"|[^\s,}]+)/gi,
      "$1$2[redacted]",
    )
    .replace(/\/(?:Users|home|private|var|tmp)\/[^\s"']+/g, "[local-path]")
    .replace(/\b[A-Z]:\\(?:Users|Temp|Windows)\\[^\s"']+/gi, "[local-path]");
}

function enqueueConsoleMessage(fnName: ConsoleFnName, message: string) {
  if (!import.meta.env.DEV && (fnName === "log" || fnName === "debug")) {
    return;
  }
  const queue = pendingMessages.get(fnName) ?? [];
  if (queue.length < MAX_BATCH_MESSAGES) {
    queue.push(message.slice(0, MAX_BATCH_BYTES));
  }
  pendingMessages.set(fnName, queue);
  if (batchTimer === undefined) {
    batchTimer = window.setTimeout(flushConsoleMessages, BATCH_DELAY_MS);
  }
}

function flushConsoleMessages() {
  batchTimer = undefined;
  for (const [fnName, messages] of pendingMessages) {
    pendingMessages.delete(fnName);
    if (messages.length === 0) continue;
    const logger = consoleForwarders[fnName];
    const combined = messages.join("\n").slice(0, MAX_BATCH_BYTES);
    void logger(combined).catch(() => {
      // Avoid logging a forwarding failure back through the same patched
      // console and recursively filling the queue.
    });
  }
}

function forwardConsole(fnName: ConsoleFnName, _logger: LogForwarder) {
  const original = console[fnName].bind(console);
  console[fnName] = (...args: unknown[]) => {
    original(...args);
    const message = formatConsoleArgs(args);
    enqueueConsoleMessage(fnName, message);
  };
}

export function installConsoleLogForwarding() {
  if (installed || !isTauriRuntime()) {
    return;
  }
  installed = true;
  for (const [fnName, logger] of Object.entries(consoleForwarders)) {
    forwardConsole(fnName as ConsoleFnName, logger);
  }
}
