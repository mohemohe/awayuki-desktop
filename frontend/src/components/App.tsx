import React from "react";
import { listen } from "@tauri-apps/api/event";
import { Loader2, X } from "lucide-react";
import { hasTauriRuntime } from "../api/tauri";
import { LoginView } from "./auth/LoginView";
import { ConfirmationDialog } from "./common/ConfirmationDialog";
import { MediaPreviewOverlay } from "./media/MediaPreviewOverlay";
import { SettingsView } from "./settings/SettingsView";
import { WorkspaceView } from "./workspace/WorkspaceView";
import { useAppStore } from "../store/appStore";
import type {
  SidecarSettings,
  StartupSyncEvent,
  TimelineStreamEvent,
} from "../types/app";
import { t } from "../i18n";

const SIDECAR_MIN_WIDTH = 160;
const SIDECAR_DEFAULT_WIDTH = 500;
const TOAST_WINDOW_EDGE_GAP = 16;

export function App() {
  const snapshot = useAppStore((state) => state.snapshot);
  const error = useAppStore((state) => state.error);
  const settingsOpen = useAppStore((state) => state.settingsOpen);
  const loginOpen = useAppStore((state) => state.loginOpen);
  const mediaPreview = useAppStore((state) => state.mediaPreview);
  const loadSnapshot = useAppStore((state) => state.loadSnapshot);
  const dismissError = React.useCallback(() => {
    useAppStore.setState({ error: undefined });
  }, []);

  React.useEffect(() => {
    void loadSnapshot();
  }, [loadSnapshot]);

  React.useEffect(() => {
    if (!hasTauriRuntime()) return;
    let disposed = false;
    const unlisteners: Array<() => void> = [];
    void listen<TimelineStreamEvent>("timeline-stream-event", (event) => {
      useAppStore.getState().applyStreamEvent(event.payload);
    }).then((dispose) => {
      if (disposed) {
        dispose();
      } else {
        unlisteners.push(dispose);
      }
    });
    void listen<StartupSyncEvent>("timeline-startup-sync-complete", (event) => {
      const { snapshot, loadTimeline, loadStatusBar } = useAppStore.getState();
      useAppStore.setState({ statusMessage: event.payload.message });
      void loadStatusBar();
      if (!snapshot || event.payload.kind !== "complete") return;
      void Promise.all(
        snapshot.columns.map((column) => loadTimeline(column, true)),
      );
    }).then((dispose) => {
      if (disposed) {
        dispose();
      } else {
        unlisteners.push(dispose);
      }
    });
    return () => {
      disposed = true;
      unlisteners.forEach((unlisten) => unlisten());
    };
  }, []);

  if (!snapshot) {
    return (
      <main className="grid h-screen place-items-center bg-base-100 text-base-content">
        <div className="flex items-center gap-3 text-sm text-subtext0">
          <Loader2 className="h-4 w-4 animate-spin" />
          {t("Loading Awayuki")}
        </div>
      </main>
    );
  }

  const toastRightInset = notificationRightInset({
    sidecars:
      !settingsOpen &&
      !loginOpen &&
      snapshot.accounts.length > 0 &&
      !mediaPreview
        ? snapshot.settings.sidecars
        : undefined,
  });

  return (
    <main className="h-screen overflow-hidden bg-base-100 text-base-content">
      {loginOpen || snapshot.accounts.length === 0 ? (
        <LoginView cancellable={snapshot.accounts.length > 0} />
      ) : settingsOpen ? (
        <SettingsView />
      ) : (
        <WorkspaceView />
      )}
      <ConfirmationDialog />
      {mediaPreview ? <MediaPreviewOverlay preview={mediaPreview} /> : null}
      {error ? (
        <div
          className="toast toast-end toast-bottom z-50"
          style={{ insetInlineEnd: `${toastRightInset}px` }}
        >
          <div className="alert alert-error max-w-xl items-start gap-3 text-xs">
            <span className="min-w-0 break-words">{error}</span>
            <button
              type="button"
              className="btn btn-circle btn-ghost btn-xs shrink-0"
              aria-label={t("Close")}
              title={t("Close")}
              onClick={dismissError}
            >
              <X className="h-3.5 w-3.5" />
            </button>
          </div>
        </div>
      ) : null}
    </main>
  );
}

function notificationRightInset({
  sidecars,
}: {
  sidecars?: SidecarSettings;
}) {
  if (!sidecars) return TOAST_WINDOW_EDGE_GAP;
  const sidecarWidth = sidecars.entries
    .filter((entry) => isSupportedSidecarUrl(entry.url))
    .reduce(
      (total, entry) => total + normalizeSidecarWidth(entry.width),
      0,
    );
  return TOAST_WINDOW_EDGE_GAP + sidecarWidth;
}

function normalizeSidecarWidth(width: number) {
  const parsed = Number(width);
  if (!Number.isFinite(parsed) || parsed <= 0) return SIDECAR_DEFAULT_WIDTH;
  return Math.max(SIDECAR_MIN_WIDTH, Math.floor(parsed));
}

function isSupportedSidecarUrl(url: string) {
  return url.startsWith("https://") || url.startsWith("http://");
}
