import React from "react";
import {
  invokeTypedCommand,
  invokeTypedReadCommand,
} from "../../api/tauri";
import { t } from "../../i18n";
import {
  loadPluginSnapshot,
  publishPluginSnapshot,
  subscribePluginSnapshot,
  type PluginInfo,
  type PluginSnapshot,
} from "../plugins/pluginSnapshot";

type PluginAction =
  | "refresh"
  | "open-directory"
  | "reload-all"
  | `reload:${string}`
  | `unload:${string}`;

export function PluginSettingsPanel() {
  const [snapshot, setSnapshot] = React.useState<PluginSnapshot>();
  const [selectedPluginId, setSelectedPluginId] = React.useState<string>();
  const [pending, setPending] = React.useState<PluginAction>();
  const [error, setError] = React.useState<string>();

  const applySnapshot = React.useCallback((next: PluginSnapshot) => {
    setSnapshot(next);
    setSelectedPluginId((current) =>
      current && next.plugins.some((plugin) => plugin.id === current)
        ? current
        : next.plugins[0]?.id,
    );
  }, []);

  React.useEffect(() => {
    let active = true;
    const unsubscribe = subscribePluginSnapshot((next) => {
      if (active) applySnapshot(next);
    });
    void loadPluginSnapshot()
      .then((next) => {
        if (active) applySnapshot(next);
      })
      .catch((cause) => {
        if (active) setError(String(cause));
      });
    return () => {
      active = false;
      unsubscribe();
    };
  }, [applySnapshot]);

  const run = async (
    action: PluginAction,
    operation: () => Promise<PluginSnapshot>,
  ) => {
    if (pending) return;
    setPending(action);
    setError(undefined);
    try {
      const next = await operation();
      applySnapshot(next);
      publishPluginSnapshot(next);
    } catch (cause) {
      setError(String(cause));
    } finally {
      setPending(undefined);
    }
  };

  const openPluginDirectory = async () => {
    if (pending || !snapshot) return;
    setPending("open-directory");
    setError(undefined);
    try {
      await invokeTypedCommand("open_plugin_directory");
    } catch (cause) {
      setError(String(cause));
    } finally {
      setPending(undefined);
    }
  };

  const selectedPlugin = snapshot?.plugins.find(
    (plugin) => plugin.id === selectedPluginId,
  );
  const busy = pending !== undefined;

  return (
    <div className="space-y-5 text-sm">
      <div className="flex flex-wrap items-start justify-between gap-4">
        <div className="min-w-0 flex-1 basis-48">
          <div className="text-xs text-subtext0">{t("Plugin directory")}</div>
          <div className="mt-1 break-all font-mono text-xs text-text">
            {snapshot?.directory ?? "—"}
          </div>
          <button
            className="btn btn-secondary btn-sm mt-2 h-8 min-h-8 px-4 text-sm font-normal"
            disabled={busy || !snapshot}
            onClick={() => void openPluginDirectory()}
            type="button"
          >
            {t("Open directory")}
          </button>
        </div>
        <div className="flex shrink-0 gap-2">
          <button
            aria-label={t("Refresh")}
            className="btn btn-secondary btn-sm h-8 min-h-8 px-4 text-sm font-normal"
            disabled={busy}
            onClick={() =>
              void run("refresh", () =>
                invokeTypedReadCommand("plugin_snapshot"),
              )
            }
            type="button"
          >
            {t("Refresh")}
          </button>
          <button
            className="btn btn-secondary btn-sm h-8 min-h-8 px-4 text-sm font-normal"
            disabled={busy}
            onClick={() =>
              void run("reload-all", () => invokeTypedCommand("reload_plugins"))
            }
            type="button"
          >
            {t("Reload all")}
          </button>
        </div>
      </div>

      {error ? (
        <p className="text-xs text-red" role="alert">
          {error}
        </p>
      ) : null}

      {!snapshot ? (
        <p className="text-sm text-subtext0" role="status">
          {t("Loading...")}
        </p>
      ) : snapshot.plugins.length === 0 ? (
        <p className="text-sm text-subtext0">{t("No plugins found")}</p>
      ) : (
        <div className="space-y-2">
          {snapshot.plugins.map((plugin) => {
            const selected = plugin.id === selectedPluginId;
            const unloaded = plugin.state.toLowerCase() === "unloaded";
            return (
              <article
                className={`rounded border ${selected ? "border-blue/70 bg-base-200" : "border-surface0 bg-base-100"}`}
                data-plugin-id={plugin.id}
                key={plugin.id}
              >
                <div className="flex min-w-0 items-center gap-2 p-3">
                  <button
                    aria-pressed={selected}
                    className="min-w-0 flex-1 text-left"
                    onClick={() => setSelectedPluginId(plugin.id)}
                    type="button"
                  >
                    <span className="flex min-w-0 items-center gap-2">
                      <span className="truncate font-medium text-text">
                        {plugin.id}
                      </span>
                      <PluginState state={plugin.state} />
                      {plugin.version == null ? null : (
                        <span className="shrink-0 text-xs text-overlay0">
                          v{plugin.version}
                        </span>
                      )}
                    </span>
                    {plugin.fileName === plugin.id ? null : (
                      <span className="mt-1 block truncate font-mono text-xs text-subtext0">
                        {plugin.fileName}
                      </span>
                    )}
                  </button>
                  <button
                    aria-label={`${t("Reload")} ${plugin.id}`}
                    className="btn btn-secondary btn-xs h-7 min-h-7 px-3 font-normal"
                    disabled={busy}
                    onClick={() =>
                      void run(`reload:${plugin.id}`, () =>
                        invokeTypedCommand("reload_plugin", {
                          request: { pluginId: plugin.id },
                        }),
                      )
                    }
                    type="button"
                  >
                    {t("Reload")}
                  </button>
                  <button
                    aria-label={`${t("Unload")} ${plugin.id}`}
                    className="btn btn-secondary btn-xs h-7 min-h-7 px-3 font-normal"
                    disabled={busy || unloaded}
                    onClick={() =>
                      void run(`unload:${plugin.id}`, () =>
                        invokeTypedCommand("unload_plugin", {
                          request: { pluginId: plugin.id },
                        }),
                      )
                    }
                    type="button"
                  >
                    {t("Unload")}
                  </button>
                </div>
                {plugin.error ? (
                  <p
                    className="border-t border-surface0 px-3 py-2 text-xs text-red"
                    role="alert"
                  >
                    {plugin.error}
                  </p>
                ) : null}
              </article>
            );
          })}
        </div>
      )}

      {selectedPlugin ? <PluginConsole plugin={selectedPlugin} /> : null}
    </div>
  );
}

function PluginState({ state }: { state: string }) {
  const normalized = state.toLowerCase();
  const label =
    normalized === "loaded"
      ? t("Loaded")
      : normalized === "unloaded"
        ? t("Unloaded")
        : normalized === "error"
          ? t("Error")
          : state;
  const color =
    normalized === "loaded"
      ? "text-green"
      : normalized === "error"
        ? "text-red"
        : "text-subtext0";
  return <span className={`shrink-0 text-xs ${color}`}>{label}</span>;
}

function PluginConsole({ plugin }: { plugin: PluginInfo }) {
  const log = plugin.logs
    .map(({ timestamp, level, message }) => `${timestamp} [${level}] ${message}`)
    .join("\n");
  return (
    <section aria-labelledby="plugin-console-heading">
      <h2
        className="mb-2 text-sm font-medium text-text"
        id="plugin-console-heading"
      >
        {t("Console log")} · {plugin.id}
      </h2>
      <pre
        aria-label={t("Console log")}
        className="max-h-80 overflow-auto whitespace-pre rounded border border-surface0 bg-base-300 p-3 font-mono text-xs text-subtext0"
      >
        {log || t("No console messages")}
      </pre>
    </section>
  );
}
