import {
  invokeTypedCommand,
  invokeTypedReadCommand,
} from "../../api/tauri";
import type {
  PluginComposeButton,
  PluginInfo,
  PluginLogEntry,
  PluginSnapshot,
} from "../../types/app";

export type {
  PluginComposeButton,
  PluginInfo,
  PluginLogEntry,
  PluginSnapshot,
};

export const pluginsChangedEventName = "awayuki:plugins-changed";

export type PluginComposeButtonRequest = {
  pluginId: string;
  buttonId: string;
  generation: number;
  compose: unknown;
};

let cachedSnapshot: PluginSnapshot | null = null;
let pendingLoad: Promise<PluginSnapshot> | null = null;

export function currentPluginSnapshot() {
  return cachedSnapshot;
}

export function publishPluginSnapshot(snapshot: PluginSnapshot) {
  if (
    cachedSnapshot &&
    cachedSnapshot.directory === snapshot.directory &&
    snapshot.revision < cachedSnapshot.revision
  ) {
    return cachedSnapshot;
  }
  cachedSnapshot = snapshot;
  if (typeof window !== "undefined") {
    window.dispatchEvent(
      new CustomEvent<PluginSnapshot>(pluginsChangedEventName, {
        detail: snapshot,
      }),
    );
  }
  return snapshot;
}

export function subscribePluginSnapshot(
  listener: (snapshot: PluginSnapshot) => void,
) {
  if (typeof window === "undefined") return () => undefined;
  const handleChange = (event: Event) => {
    listener((event as CustomEvent<PluginSnapshot>).detail);
  };
  window.addEventListener(pluginsChangedEventName, handleChange);
  return () => window.removeEventListener(pluginsChangedEventName, handleChange);
}

export function loadPluginSnapshot() {
  if (!pendingLoad) {
    pendingLoad = invokeTypedReadCommand("plugin_snapshot")
      .then(publishPluginSnapshot)
      .finally(() => {
        pendingLoad = null;
      });
  }
  return pendingLoad;
}

export async function invokePluginComposeButton(
  request: PluginComposeButtonRequest,
): Promise<unknown> {
  try {
    return await invokeTypedCommand("invoke_plugin_compose_button", {
      request,
    });
  } finally {
    // Invocation can append plugin console output. Refresh it without making
    // applying the returned compose draft wait on the diagnostic snapshot.
    void loadPluginSnapshot().catch(() => undefined);
  }
}
