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
  return args.map(formatConsoleValue).join(" ");
}

function forwardConsole(fnName: ConsoleFnName, logger: LogForwarder) {
  const original = console[fnName].bind(console);
  console[fnName] = (...args: unknown[]) => {
    original(...args);
    const message = formatConsoleArgs(args);
    void logger(message).catch((logError) => {
      original("[frontend-log-forwarding]", logError);
    });
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
