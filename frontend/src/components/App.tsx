import React from "react";
import { listen } from "@tauri-apps/api/event";
import { Loader2, X } from "lucide-react";
import { hasTauriRuntime } from "../api/tauri";
import { ConfirmationDialog } from "./common/ConfirmationDialog";
import { WorkspaceView } from "./workspace/WorkspaceView";
import { useAppStore, type BootState } from "../store/appStore";
import {
  isSupportedSidecarUrl,
  normalizeSidecarWidth,
} from "../domain/sidecar";
import type {
  AppStartupProgressEvent,
  SidecarSettings,
  StartupSyncEvent,
  TimelineStreamEvent,
} from "../types/app";
import { t } from "../i18n";

const TOAST_WINDOW_EDGE_GAP = 16;
const LoginView = React.lazy(() =>
  import("./auth/LoginView").then((module) => ({ default: module.LoginView })),
);
const SettingsView = React.lazy(() =>
  import("./settings/SettingsView").then((module) => ({
    default: module.SettingsView,
  })),
);
const MediaPreviewOverlay = React.lazy(() =>
  import("./media/MediaPreviewOverlay").then((module) => ({
    default: module.MediaPreviewOverlay,
  })),
);

export function App() {
  const boot = useAppStore((state) => state.boot);
  const snapshot = useAppStore((state) => state.snapshot);
  const error = useAppStore((state) => state.error);
  const settingsOpen = useAppStore((state) => state.settingsOpen);
  const loginOpen = useAppStore((state) => state.loginOpen);
  const mediaPreview = useAppStore((state) => state.mediaPreview);
  const loadSnapshot = useAppStore((state) => state.loadSnapshot);
  const applyStartupProgress = useAppStore(
    (state) => state.applyStartupProgress,
  );
  const dismissError = React.useCallback(() => {
    useAppStore.setState({ error: undefined });
  }, []);

  React.useEffect(() => {
    if (!hasTauriRuntime()) {
      void loadSnapshot();
      return;
    }

    let disposed = false;
    let unlisten: (() => void) | undefined;
    void listen<AppStartupProgressEvent>(
      "app-startup-progress",
      (event) => {
        applyStartupProgress(event.payload);
      },
    ).then(
      (dispose) => {
        if (disposed) {
          dispose();
          return;
        }
        unlisten = dispose;
        // Subscribe first so the initial database stage cannot be missed.
        void loadSnapshot();
      },
      () => {
        // Progress reporting is diagnostic; snapshot loading remains usable if
        // the event channel itself cannot be registered.
        if (!disposed) void loadSnapshot();
      },
    );

    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [applyStartupProgress, loadSnapshot]);

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
        snapshot.columns.map((column) =>
          loadTimeline(
            column,
            // Startup sync already refreshed every signed-in source for the
            // Unified timelines. Reload those columns from the shared SQLite
            // cache instead of issuing the same provider requests a second
            // time. Account-bound timelines retain their explicit refresh.
            !["home", "public", "notification"].includes(column.columnType),
          ),
        ),
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
    if (boot.status === "error") {
      return (
        <main className="grid h-screen place-items-center bg-base-100 px-6 text-base-content">
          <section
            className="flex max-w-md flex-col items-center gap-3 text-center"
            role="alert"
          >
            <h1 className="text-lg text-text">{t("Awayuki could not start")}</h1>
            <p className="text-sm text-subtext0">
              {t("Awayuki could not restore its local data and accounts.")}
            </p>
            <p className="text-xs text-overlay1">
              {bootStageLabel(boot)}
            </p>
            <button
              type="button"
              className="btn btn-secondary btn-sm mt-2 h-8 min-h-8 px-4 text-sm font-normal"
              onClick={() => void loadSnapshot()}
            >
              {t("Retry")}
            </button>
          </section>
        </main>
      );
    }
    return (
      <main className="grid h-screen place-items-center bg-base-100 text-base-content">
        <div className="flex flex-col items-center gap-2" role="status">
          <div className="flex items-center gap-3 text-sm text-subtext0">
            <Loader2 className="h-4 w-4 animate-spin" />
            {bootStageLabel(boot)}
          </div>
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
      <React.Suspense fallback={<FeatureLoadingFallback />}>
        {loginOpen || snapshot.accounts.length === 0 ? (
          <LoginView cancellable={snapshot.accounts.length > 0} />
        ) : settingsOpen ? (
          <SettingsView />
        ) : (
          <WorkspaceView />
        )}
      </React.Suspense>
      <ConfirmationDialog />
      {mediaPreview ? (
        <React.Suspense fallback={null}>
          <MediaPreviewOverlay preview={mediaPreview} />
        </React.Suspense>
      ) : null}
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

function FeatureLoadingFallback() {
  return (
    <div className="grid h-full place-items-center" role="status">
      <Loader2 className="h-4 w-4 animate-spin" aria-hidden="true" />
      <span className="sr-only">{t("Loading")}</span>
    </div>
  );
}

function bootStageLabel(boot: BootState) {
  if (boot.stage === "snapshot" && boot.backendProgress) {
    switch (boot.backendProgress.stage) {
      case "database":
        return t("Updating the local database. Large caches can take several minutes.");
      case "sessions":
        return t("Restoring account sessions");
      case "services":
        return t("Starting background services");
      case "ready":
        return t("Preparing Awayuki");
      case "error":
        return t("Startup initialization failed");
    }
  }

  switch (boot.stage) {
    case "snapshot":
      return t("Updating the local database. Large caches can take several minutes.");
    case "timelines":
      return t("Loading initial timelines");
    case "complete":
      return t("Loading Awayuki");
  }
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
