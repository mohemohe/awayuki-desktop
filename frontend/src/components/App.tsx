import React from "react";
import { setTheme as setNativeTheme } from "@tauri-apps/api/app";
import { listen } from "@tauri-apps/api/event";
import { Loader2, X } from "lucide-react";
import { hasTauriRuntime } from "../api/tauri";
import { ConfirmationDialog } from "./common/ConfirmationDialog";
import { ComposeOutboxDialog } from "./compose/ComposeOutboxDialog";
import { WorkspaceView } from "./workspace/WorkspaceView";
import { useAppStore, type BootState } from "../store/appStore";
import { reduceBootState } from "../store/slices/session";
import {
  isSupportedSidecarUrl,
  normalizeSidecarWidth,
} from "../domain/sidecar";
import type {
  AppStartupProgressEvent,
  ComposeOutboxUpdatedEvent,
  SidecarSettings,
  StartupSyncEvent,
  TimelineCacheCommittedEvent,
  TimelineQueryMetricsEvent,
  TimelineStreamEvent,
} from "../types/app";
import { t } from "../i18n";
import {
  markFrontendReactCommit,
  scheduleFrontendInteractiveMark,
} from "../utils/startupMetrics";
import { applyAppearanceTheme } from "../utils/theme";

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
  const [listenerAttempt, setListenerAttempt] = React.useState(0);
  const theme = snapshot?.settings.appearance?.theme ?? "Mocha";
  const dismissError = React.useCallback(() => {
    useAppStore.setState({ error: undefined });
  }, []);

  React.useLayoutEffect(() => {
    markFrontendReactCommit();
  });

  React.useLayoutEffect(() => {
    applyAppearanceTheme(theme);
    if (
      hasTauriRuntime() &&
      typeof window !== "undefined" &&
      "__TAURI_INTERNALS__" in window
    ) {
      void setNativeTheme(theme === "Latte" ? "light" : "dark").catch(
        (error) => {
          console.warn("Unable to apply the native window theme", error);
        },
      );
    }
  }, [theme]);

  React.useEffect(() => {
    if (snapshot) scheduleFrontendInteractiveMark();
  }, [snapshot]);

  React.useEffect(() => {
    if (!hasTauriRuntime()) {
      void loadSnapshot();
      return;
    }

    let disposed = false;
    const unlisteners: Array<() => void> = [];
    void Promise.allSettled([
      listen<AppStartupProgressEvent>("app-startup-progress", (event) => {
        applyStartupProgress(event.payload);
      }),
      listen<TimelineStreamEvent>("timeline-stream-event", (event) => {
        useAppStore.getState().applyStreamEvent(event.payload);
      }),
      listen<TimelineCacheCommittedEvent>("timeline-cache-committed", () => {
        useAppStore.getState().applyTimelineCacheCommit();
      }),
      listen<ComposeOutboxUpdatedEvent>("compose-outbox-updated", (event) => {
        useAppStore.getState().applyComposeOutboxUpdate(event.payload);
      }),
      listen<StartupSyncEvent>("timeline-startup-sync-complete", (event) => {
        const { snapshot, dynamicColumns, loadTimeline, loadStatusBar } =
          useAppStore.getState();
        useAppStore.setState({ statusMessage: event.payload.message });
        void loadStatusBar();
        if (!snapshot || event.payload.kind !== "complete") return;
        const regularColumns = snapshot.columns.filter(
          (column) => !["custom", "yq"].includes(column.columnType),
        );
        const hasAnalyticalColumns =
          regularColumns.length !== snapshot.columns.length ||
          dynamicColumns.some((column) =>
            ["custom", "yq"].includes(column.columnType),
          );
        void Promise.all(
          regularColumns.map((column) =>
            loadTimeline(
              column,
              // Unified timelines reload from the shared SQLite cache. The
              // active account remains only the operation source.
              !["home", "public", "notification"].includes(column.columnType),
            ),
          ),
        ).finally(() => {
          if (hasAnalyticalColumns) {
            useAppStore.getState().applyTimelineCacheCommit();
          }
        });
      }),
      listen<TimelineQueryMetricsEvent>("timeline-query-metrics", (event) => {
        if (!event.payload.slow) return;
        useAppStore.setState({
          statusMessage: t("timeline.yqSlow", {
            scanned: event.payload.scannedCount,
            duration: event.payload.durationMs,
          }),
        });
      }),
    ]).then((registrations) => {
      const successful = registrations.flatMap((registration) =>
        registration.status === "fulfilled" ? [registration.value] : [],
      );
      if (disposed) {
        successful.forEach((unlisten) => unlisten());
        return;
      }
      if (registrations.some((registration) => registration.status === "rejected")) {
        successful.forEach((unlisten) => unlisten());
        useAppStore.setState((state) => ({
          boot: reduceBootState(state.boot, {
            type: "listenerRegistrationFailed",
            error: "Application event listeners could not be registered",
          }),
        }));
        return;
      }
      unlisteners.push(...successful);
      // Subscribe to every critical channel before the background database
      // initializer can emit progress or streaming events.
      void loadSnapshot();
    });
    return () => {
      disposed = true;
      unlisteners.forEach((unlisten) => unlisten());
    };
  }, [applyStartupProgress, listenerAttempt, loadSnapshot]);

  React.useEffect(() => {
    if (snapshot) void useAppStore.getState().loadComposeOutbox();
  }, [snapshot]);

  const retryStartup = React.useCallback(() => {
    if (boot.stage === "listeners") {
      setListenerAttempt((attempt) => attempt + 1);
      return;
    }
    void loadSnapshot();
  }, [boot.stage, loadSnapshot]);

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
              onClick={retryStartup}
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
      <ComposeOutboxDialog />
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
    if (boot.backendProgress.status === "error") {
      switch (boot.backendProgress.stage) {
        case "database":
          return t("The portable database could not be initialized");
        case "settings":
          return t("Application settings could not be restored");
        case "sessions":
          return t("Account sessions could not be restored");
        case "services":
          return t("Background services could not be started");
      }
    }
    switch (boot.backendProgress.stage) {
      case "database":
        return t("Updating the local database. Large caches can take several minutes.");
      case "settings":
        return t("Restoring application settings");
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
    case "listeners":
      return t("Registering application event listeners");
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
