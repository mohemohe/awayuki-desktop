import React from "react";
import { ChevronUp, Home, RefreshCw, Search } from "lucide-react";
import { LogicalPosition, LogicalSize } from "@tauri-apps/api/dpi";
import { Webview } from "@tauri-apps/api/webview";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { ComposeArea } from "../compose/ComposeArea";
import { Tab, TabList } from "../primitives/Tabs";
import { StatusBar } from "../status/StatusBar";
import { TimelineArea } from "../timeline/TimelineArea";
import { hasTauriRuntime, invokeTypedCommand } from "../../api/tauri";
import {
  SIDECAR_MIN_WIDTH,
  SidecarLifecycleManager,
  SidecarStyleRetryScheduler,
  effectiveSidecarUserStyle,
  normalizeSidecarSettings,
  sidecarWebviewLabel,
  type SidecarOperation,
} from "../../domain/sidecar";
import { useAppStore } from "../../store/appStore";
import type { PerformanceSettings, SidecarEntry } from "../../types/app";
import { getClientPlatform } from "../../utils/browser";
import { groupColumnsByPane } from "../../utils/columns";
import { t } from "../../i18n";

export function WorkspaceView() {
  const snapshot = useAppStore((state) => state.snapshot);
  const activeTabs = useAppStore((state) => state.activeTabs);
  const dynamicColumns = useAppStore((state) => state.dynamicColumns);
  const sidecarsVisible = useAppStore(
    (state) => state.mediaPreview == null && !state.composeOutboxOpen,
  );
  const sidecars = React.useMemo(
    () => normalizeSidecarSettings(snapshot?.settings.sidecars),
    [snapshot?.settings.sidecars],
  );
  const sidecarHiddenTabBehavior =
    snapshot?.settings.performance.sidecar_hidden_tab_behavior ?? "Keep";
  if (!snapshot) return null;

  const panes = groupColumnsByPane([...snapshot.columns, ...dynamicColumns]);

  return (
    <div className="flex h-screen min-h-0 flex-col overflow-hidden">
      <CustomTitleBar />
      <ComposeArea />
      <div className="flex min-h-0 flex-1 overflow-hidden">
        <TimelineArea panes={panes} activeTabs={activeTabs} />
        <SidecarRegion
          sidecars={sidecars.entries}
          visible={sidecarsVisible}
          hiddenTabBehavior={sidecarHiddenTabBehavior}
        />
      </div>
      <StatusBar />
    </div>
  );
}

function SidecarRegion({
  sidecars,
  visible,
  hiddenTabBehavior,
}: {
  sidecars: SidecarEntry[];
  visible: boolean;
  hiddenTabBehavior: PerformanceSettings["sidecar_hidden_tab_behavior"];
}) {
  const containerRef = React.useRef<HTMLDivElement | null>(null);
  const webviews = React.useRef<Partial<Record<string, SidecarWebviewState>>>(
    {},
  );
  const lifecycle = React.useRef(new SidecarLifecycleManager());
  const styleRetries = React.useRef(new SidecarStyleRetryScheduler());
  const requestSyncRef = React.useRef<() => void>(() => undefined);
  const sidecarsRef = React.useRef(sidecars);
  const visibleRef = React.useRef(visible);
  const hiddenTabBehaviorRef = React.useRef(hiddenTabBehavior);
  const [activeSidecarId, setActiveSidecarId] = React.useState<string | null>(
    () => sidecars[0]?.id ?? null,
  );
  const activeSidecar =
    sidecars.find((sidecar) => sidecar.id === activeSidecarId) ??
    sidecars[0] ??
    null;
  const activeSidecarIdRef = React.useRef(activeSidecar?.id ?? null);
  const mountedRef = React.useRef(true);
  const syncFrameRef = React.useRef<number | null>(null);
  const syncPromiseRef = React.useRef<Promise<void> | null>(null);
  const syncAgainRef = React.useRef(false);
  const [errors, setErrors] = React.useState<Record<string, string>>({});
  sidecarsRef.current = sidecars;
  visibleRef.current = visible;
  hiddenTabBehaviorRef.current = hiddenTabBehavior;
  activeSidecarIdRef.current = activeSidecar?.id ?? null;

  React.useEffect(() => {
    setActiveSidecarId((current) =>
      current && sidecars.some((sidecar) => sidecar.id === current)
        ? current
        : sidecars[0]?.id ?? null,
    );
  }, [sidecars]);

  const reportFailure = React.useCallback(
    (sidecarId: string, operation: SidecarOperation, error: unknown) => {
      if (
        !mountedRef.current ||
        !lifecycle.current.isCurrent(operation)
      ) {
        return;
      }
      lifecycle.current.transition(operation, "failed");
      const message = formatSidecarError(error);
      setErrors((current) => ({ ...current, [sidecarId]: message }));
      console.warn("Sidecar operation failed", sidecarId, message);
    },
    [],
  );

  const closeSidecar = React.useCallback(
    async (sidecarId: string) => {
      styleRetries.current.remove(sidecarId);
      const operation = lifecycle.current.begin(sidecarId, "closing");
      try {
        if (hasTauriRuntime()) {
          await invokeTypedCommand("close_sidecar_webview", { sidecarId });
        }
        if (!lifecycle.current.isCurrent(operation)) return false;
        delete webviews.current[sidecarId];
        lifecycle.current.remove(operation);
        if (mountedRef.current) {
          setErrors((current) => clearSidecarError(current, sidecarId));
        }
        return true;
      } catch (error) {
        reportFailure(sidecarId, operation, error);
        return false;
      }
    },
    [reportFailure],
  );

  const closeCreatedSidecar = React.useCallback(async (sidecarId: string) => {
    if (!hasTauriRuntime()) return;
    try {
      await invokeTypedCommand("close_sidecar_webview", { sidecarId });
    } catch (error) {
      console.warn("Failed to clean up sidecar webview", sidecarId, error);
    }
  }, []);

  const operationStillMatches = React.useCallback(
    (operation: SidecarOperation, sidecar: SidecarEntry) =>
      mountedRef.current &&
      lifecycle.current.isCurrent(operation) &&
      sidecarsRef.current.some(
        (current) =>
          current.id === sidecar.id && current.url === sidecar.url,
      ) &&
      containerRef.current != null,
    [],
  );

  const applySidecarLayout = React.useCallback(
    async (
      sidecar: SidecarEntry,
      state: SidecarWebviewState,
      operation: SidecarOperation,
    ) => {
      const element = containerRef.current;
      if (!element) return false;
      const rect = element.getBoundingClientRect();
      if (rect.width <= 0 || rect.height <= 0) return false;
      const targetVisible =
        visibleRef.current && activeSidecarIdRef.current === sidecar.id;

      if (!targetVisible && state.visible) {
        await state.webview.hide();
        state.visible = false;
      }
      if (!operationStillMatches(operation, sidecar)) return false;

      await state.webview.setPosition(
        new LogicalPosition(rect.left, rect.top),
      );
      if (!operationStillMatches(operation, sidecar)) return false;

      await state.webview.setSize(
        new LogicalSize(rect.width, rect.height),
      );
      if (!operationStillMatches(operation, sidecar)) return false;

      const userStyle = effectiveSidecarUserStyle(sidecar);
      if (state.userStyle !== userStyle) {
        try {
          await invokeTypedCommand("inject_sidecar_user_style", {
            sidecarId: sidecar.id,
            userStyle,
          });
        } catch (error) {
          styleRetries.current.retry(sidecar.id, () => {
            if (mountedRef.current) requestSyncRef.current();
          });
          throw error;
        }
        if (!operationStillMatches(operation, sidecar)) return false;
        state.userStyle = userStyle;
        styleRetries.current.succeed(sidecar.id);
      }
      const shouldBeVisible =
        visibleRef.current && activeSidecarIdRef.current === sidecar.id;
      if (shouldBeVisible && !state.visible) {
        await state.webview.show();
        state.visible = true;
      } else if (!shouldBeVisible && state.visible) {
        await state.webview.hide();
        state.visible = false;
      }
      if (!operationStillMatches(operation, sidecar)) return false;
      lifecycle.current.transition(
        operation,
        state.visible ? "visible" : "ready",
      );
      return true;
    },
    [operationStillMatches],
  );

  const createSidecar = React.useCallback(
    async (sidecar: SidecarEntry) => {
      const element = containerRef.current;
      if (!element) return;
      const rect = element.getBoundingClientRect();
      if (rect.width <= 0 || rect.height <= 0) return;

      const operation = lifecycle.current.begin(sidecar.id, "creating");
      let backendCreated = false;
      try {
        await invokeTypedCommand("create_sidecar_webview", {
          request: {
            sidecarId: sidecar.id,
            url: sidecar.url,
            userStyle: effectiveSidecarUserStyle(sidecar),
            x: rect.left,
            y: rect.top,
            width: rect.width,
            height: rect.height,
          },
        });
        backendCreated = true;
        if (!operationStillMatches(operation, sidecar)) {
          await closeCreatedSidecar(sidecar.id);
          return;
        }

        const webview = await Webview.getByLabel(
          sidecarWebviewLabel(sidecar.id),
        );
        if (!webview) {
          throw new Error(
            `Sidecar WebView not found: ${sidecarWebviewLabel(sidecar.id)}`,
          );
        }
        if (!operationStillMatches(operation, sidecar)) {
          await closeCreatedSidecar(sidecar.id);
          return;
        }

        const state: SidecarWebviewState = {
          webview,
          url: sidecar.url,
          userStyle: effectiveSidecarUserStyle(sidecar),
          visible: false,
        };
        webviews.current[sidecar.id] = state;
        if (!(await applySidecarLayout(sidecar, state, operation))) {
          delete webviews.current[sidecar.id];
          await closeCreatedSidecar(sidecar.id);
          return;
        }
        setErrors((current) => clearSidecarError(current, sidecar.id));
      } catch (error) {
        delete webviews.current[sidecar.id];
        if (backendCreated) await closeCreatedSidecar(sidecar.id);
        reportFailure(sidecar.id, operation, error);
      }
    },
    [
      applySidecarLayout,
      closeCreatedSidecar,
      operationStillMatches,
      reportFailure,
    ],
  );

  const syncOnce = React.useCallback(async () => {
    const currentSidecars = sidecarsRef.current;
    const currentIds = new Set(currentSidecars.map((sidecar) => sidecar.id));
    const knownIds = new Set([
      ...Object.keys(webviews.current),
      ...lifecycle.current.ids(),
    ]);
    for (const id of knownIds) {
      if (!currentIds.has(id)) await closeSidecar(id);
    }

    if (!mountedRef.current || !hasTauriRuntime()) return;
    if (!visibleRef.current) {
      for (const [id, state] of Object.entries(webviews.current)) {
        if (!state) continue;
        // Native visibility can drift from the cached flag after WebKit or
        // Tauri lifecycle events, so blocking overlays must reassert hide.
        const operation = lifecycle.current.begin(id, "ready");
        try {
          await state.webview.hide();
          if (!lifecycle.current.isCurrent(operation)) continue;
          state.visible = false;
          lifecycle.current.transition(operation, "ready");
        } catch (error) {
          reportFailure(id, operation, error);
        }
      }
      return;
    }

    const activeId = activeSidecarIdRef.current;
    const discardHiddenTabs = hiddenTabBehaviorRef.current === "Discard";
    if (discardHiddenTabs) {
      for (const id of knownIds) {
        if (id !== activeId && currentIds.has(id)) await closeSidecar(id);
      }
    }
    const layoutOrder = discardHiddenTabs
      ? currentSidecars.filter((sidecar) => sidecar.id === activeId)
      : [
          ...currentSidecars.filter((sidecar) => sidecar.id !== activeId),
          ...currentSidecars.filter((sidecar) => sidecar.id === activeId),
        ];
    for (const sidecar of layoutOrder) {
      if (!mountedRef.current) return;
      let state = webviews.current[sidecar.id];
      if (state && state.url !== sidecar.url) {
        if (!(await closeSidecar(sidecar.id))) continue;
        state = undefined;
      }

      if (!state) {
        await createSidecar(sidecar);
        continue;
      }

      const operation = lifecycle.current.begin(
        sidecar.id,
        activeSidecarIdRef.current === sidecar.id ? "visible" : "ready",
      );
      try {
        await applySidecarLayout(sidecar, state, operation);
        if (lifecycle.current.isCurrent(operation)) {
          setErrors((current) => clearSidecarError(current, sidecar.id));
        }
      } catch (error) {
        reportFailure(sidecar.id, operation, error);
      }
    }
  }, [applySidecarLayout, closeSidecar, createSidecar, reportFailure]);

  const drainSync = React.useCallback(async () => {
    if (syncPromiseRef.current) {
      syncAgainRef.current = true;
      return syncPromiseRef.current;
    }
    const promise = (async () => {
      do {
        syncAgainRef.current = false;
        await syncOnce();
      } while (mountedRef.current && syncAgainRef.current);
    })();
    syncPromiseRef.current = promise;
    try {
      await promise;
    } finally {
      if (syncPromiseRef.current === promise) syncPromiseRef.current = null;
    }
  }, [syncOnce]);

  const requestSync = React.useCallback(() => {
    if (!mountedRef.current) return;
    syncAgainRef.current = true;
    // A child WebView is composited above the main HTML WebView. Suppress it
    // immediately so it cannot paint over a media preview or outbox dialog.
    if (!visibleRef.current && syncFrameRef.current !== null) {
      window.cancelAnimationFrame(syncFrameRef.current);
      syncFrameRef.current = null;
    }
    if (syncPromiseRef.current) return;
    if (!visibleRef.current) {
      void drainSync().catch((error) => {
        console.warn("Failed to synchronize sidecar webviews", error);
      });
      return;
    }
    if (syncFrameRef.current !== null) return;
    syncFrameRef.current = window.requestAnimationFrame(() => {
      syncFrameRef.current = null;
      drainSync().catch((error) => {
        console.warn("Failed to synchronize sidecar webviews", error);
      });
    });
  }, [drainSync]);
  requestSyncRef.current = requestSync;

  const controlSidecar = React.useCallback(
    async (sidecar: SidecarEntry, action: "home" | "reload" | "top") => {
      const state = webviews.current[sidecar.id];
      if (!state) return;
      const operation = lifecycle.current.begin(sidecar.id, "navigating");
      try {
        if (action === "home") {
          await invokeTypedCommand("navigate_sidecar_webview", {
            sidecarId: sidecar.id,
            url: sidecar.url,
          });
        } else if (action === "reload") {
          await invokeTypedCommand("reload_sidecar_webview", {
            sidecarId: sidecar.id,
          });
        } else {
          await invokeTypedCommand("scroll_sidecar_webview_to_top", {
            sidecarId: sidecar.id,
          });
        }
        if (!lifecycle.current.isCurrent(operation)) return;
        lifecycle.current.transition(
          operation,
          state.visible ? "visible" : "ready",
        );
        setErrors((current) => clearSidecarError(current, sidecar.id));
      } catch (error) {
        reportFailure(sidecar.id, operation, error);
      }
    },
    [reportFailure],
  );

  React.useLayoutEffect(() => {
    requestSync();
  }, [activeSidecar?.id, hiddenTabBehavior, requestSync, visible]);

  React.useEffect(() => {
    requestSync();
    window.addEventListener("resize", requestSync);
    return () => window.removeEventListener("resize", requestSync);
  }, [requestSync]);

  React.useEffect(() => {
    const resizeObserver = new ResizeObserver(() => requestSync());
    const element = containerRef.current;
    if (element) resizeObserver.observe(element);
    return () => resizeObserver.disconnect();
  }, [requestSync]);

  React.useEffect(
    () => () => {
      mountedRef.current = false;
      if (syncFrameRef.current !== null) {
        window.cancelAnimationFrame(syncFrameRef.current);
        syncFrameRef.current = null;
      }
      syncAgainRef.current = false;
      styleRetries.current.cancelAll();
      const ids = new Set([
        ...lifecycle.current.ids(),
        ...Object.keys(webviews.current),
      ]);
      lifecycle.current.cancelAll();
      webviews.current = {};
      const cleanup = Promise.allSettled(
        [...ids].map((sidecarId) =>
          hasTauriRuntime()
            ? invokeTypedCommand("close_sidecar_webview", { sidecarId })
            : Promise.resolve(),
        ),
      );
      cleanup
        .then((results) => {
          results.forEach((result, index) => {
            if (result.status === "rejected") {
              console.warn(
                "Failed to close sidecar webview during cleanup",
                [...ids][index],
                result.reason,
              );
            }
          });
        })
        .catch((error) => {
          console.warn("Failed to finish sidecar cleanup", error);
        });
    },
    [],
  );

  if (!activeSidecar) return null;

  return (
    <section
      aria-label={t("Sidecar")}
      className="flex h-full shrink-0 flex-col overflow-hidden border-r border-surface0 bg-base-100"
      style={{
        width: `${Math.max(SIDECAR_MIN_WIDTH, activeSidecar.width)}px`,
      }}
    >
      <div className="flex h-8 shrink-0 items-stretch border-b border-surface0 bg-base-300">
        <div className="min-w-0 flex-1 overflow-x-auto">
          <TabList
            label={t("Sidecar")}
            className="flex h-full min-w-max items-stretch"
          >
            {sidecars.map((sidecar) => {
              const selected = sidecar.id === activeSidecar.id;
              return (
                <Tab
                  key={sidecar.id}
                  selected={selected}
                  className={`h-full min-w-20 max-w-36 border-r border-surface0 px-3 text-left text-sm ${selected ? "bg-base text-text" : "text-subtext0 hover:bg-surface0/60 hover:text-text"}`}
                  onSelect={() => setActiveSidecarId(sidecar.id)}
                  title={sidecar.name}
                >
                  <span className="block truncate">{sidecar.name}</span>
                </Tab>
              );
            })}
          </TabList>
        </div>
        <div className="flex shrink-0 items-center gap-1 px-1">
          <button
            aria-label={t("Scroll to top")}
            className="btn btn-ghost btn-xs"
            onClick={() => void controlSidecar(activeSidecar, "top")}
            title={t("Scroll to top")}
          >
            <ChevronUp className="h-3.5 w-3.5" />
          </button>
          <button
            aria-label={t("Return to sidecar URL")}
            className="btn btn-ghost btn-xs"
            onClick={() => void controlSidecar(activeSidecar, "home")}
            title={t("Return to sidecar URL")}
          >
            <Home className="h-3.5 w-3.5" />
          </button>
          <button
            aria-label={t("Reload sidecar")}
            className="btn btn-ghost btn-xs"
            onClick={() => void controlSidecar(activeSidecar, "reload")}
            title={t("Reload sidecar")}
          >
            <RefreshCw className="h-3.5 w-3.5" />
          </button>
        </div>
      </div>
      <div
        aria-label={activeSidecar.name}
        className="min-h-0 flex-1 bg-base-100"
        ref={containerRef}
        role="tabpanel"
      >
        {errors[activeSidecar.id] ? (
          <div className="grid h-full place-items-center px-3 text-center text-xs leading-relaxed text-red">
            {errors[activeSidecar.id]}
          </div>
        ) : null}
      </div>
    </section>
  );
}

type SidecarWebviewState = {
  webview: Webview;
  url: string;
  userStyle: string;
  visible: boolean;
};

function clearSidecarError(
  errors: Record<string, string>,
  sidecarId: string,
) {
  if (!(sidecarId in errors)) return errors;
  const next = { ...errors };
  delete next[sidecarId];
  return next;
}

function formatSidecarError(error: unknown) {
  if (typeof error === "string") return error;
  if (error && typeof error === "object" && "message" in error) {
    return String(error.message);
  }
  return "Failed to create sidecar WebView";
}

function CustomTitleBar() {
  const platform = getClientPlatform();
  const isMac = platform === "macos";
  const isWindows = platform === "windows";
  if (platform === "linux") return null;

  const titlePaddingClass = isMac ? "pl-20" : "pl-0";
  const headerPaddingClass = isMac ? "px-2" : "pl-2 pr-0";

  return (
    <header
      className={`relative grid h-8 shrink-0 grid-cols-[1fr_auto_1fr] items-center border-b border-surface0 bg-crust ${headerPaddingClass} text-xs text-subtext0`}
      style={
        isWindows
          ? { paddingRight: "var(--tauri-frame-controls-width, 132px)" }
          : undefined
      }
      data-tauri-drag-region
      data-tauri-frame-tb={isWindows ? "" : undefined}
    >
      <div
        className={`flex items-center gap-2 ${titlePaddingClass}`}
        data-tauri-drag-region
      >
        <span className="font-semibold text-text" data-tauri-drag-region>
          Awayuki
        </span>
      </div>
      <div className="flex justify-center" data-tauri-drag-region>
        {isMac ? null : <TitleBarSearch />}
      </div>
      <div className="flex justify-end" data-tauri-drag-region>
        {isMac ? <TitleBarSearch /> : isWindows ? null : <WindowControls />}
      </div>
    </header>
  );
}

function WindowControls() {
  const [isMaximized, setIsMaximized] = React.useState(false);

  const updateMaximizedState = React.useCallback(async () => {
    if (!hasTauriRuntime()) return;
    try {
      setIsMaximized(await getCurrentWindow().isMaximized());
    } catch (error) {
      console.warn("Window maximized state check failed", error);
    }
  }, []);

  React.useEffect(() => {
    if (!hasTauriRuntime()) return;
    const appWindow = getCurrentWindow();
    let mounted = true;
    let unlisten: (() => void) | undefined;

    const syncMaximizedState = async () => {
      try {
        const next = await appWindow.isMaximized();
        if (mounted) setIsMaximized(next);
      } catch (error) {
        console.warn("Window maximized state check failed", error);
      }
    };

    void syncMaximizedState();
    appWindow
      .onResized(() => {
        void syncMaximizedState();
      })
      .then((listener) => {
        if (mounted) {
          unlisten = listener;
        } else {
          listener();
        }
      })
      .catch((error) => {
        console.warn("Window resize listener failed", error);
      });

    return () => {
      mounted = false;
      unlisten?.();
    };
  }, []);

  const runWindowAction = React.useCallback(
    async (action: "minimize" | "toggleMaximize" | "close") => {
      if (!hasTauriRuntime()) return;
      const appWindow = getCurrentWindow();
      try {
        if (action === "minimize") {
          await appWindow.minimize();
        } else if (action === "toggleMaximize") {
          await appWindow.toggleMaximize();
          await updateMaximizedState();
        } else {
          await appWindow.close();
        }
      } catch (error) {
        console.warn("Window action failed", action, error);
      }
    },
    [updateMaximizedState],
  );
  const maximizeTitle = isMaximized ? "restore" : "maximize";

  return (
    <div className="flex h-8 items-stretch" data-tauri-drag-region>
      <button
        id="titlebar-minimize"
        type="button"
        className="grid w-11 place-items-center text-subtext0 hover:bg-surface0 hover:text-text"
        aria-label="minimize"
        title="minimize"
        onClick={() => void runWindowAction("minimize")}
      >
        <svg
          xmlns="http://www.w3.org/2000/svg"
          viewBox="0 0 16 16"
          width="16"
          height="16"
          fill="none"
          stroke="currentColor"
          strokeWidth="1.0"
        >
          <line x1="4" y1="8" x2="12" y2="8" />
        </svg>
      </button>
      <button
        id="titlebar-maximize"
        type="button"
        className="grid w-11 place-items-center text-subtext0 hover:bg-surface0 hover:text-text"
        aria-label={maximizeTitle}
        title={maximizeTitle}
        onClick={() => void runWindowAction("toggleMaximize")}
      >
        {isMaximized ? <RestoreIcon /> : <MaximizeIcon />}
      </button>
      <button
        id="titlebar-close"
        type="button"
        className="grid w-11 place-items-center text-subtext0 hover:bg-[#C42B1C] hover:text-white"
        aria-label="close"
        title="close"
        onClick={() => void runWindowAction("close")}
      >
        <svg
          xmlns="http://www.w3.org/2000/svg"
          viewBox="0 0 16 16"
          width="16"
          height="16"
          fill="none"
          stroke="currentColor"
          strokeWidth="1.0"
        >
          <line x1="4.5" y1="4.5" x2="11.5" y2="11.5" />
          <line x1="11.5" y1="4.5" x2="4.5" y2="11.5" />
        </svg>
      </button>
    </div>
  );
}

function MaximizeIcon() {
  return (
    <svg
      xmlns="http://www.w3.org/2000/svg"
      viewBox="0 0 16 16"
      width="16"
      height="16"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.0"
    >
      <rect x="4" y="4" width="8" height="8" rx="1" />
    </svg>
  );
}

function RestoreIcon() {
  return (
    <svg xmlns="http://www.w3.org/2000/svg" width="10" height="10" viewBox="0 0 10 10">
      <path
        fill="none"
        stroke="currentColor"
        strokeWidth="1"
        strokeLinecap="round"
        strokeLinejoin="round"
        d="M 4,1.5 H 7.5 C 8,1.5 8.5,2 8.5,2.5 V 6"
      />
      <rect
        x="1.5"
        y="3.5"
        width="5"
        height="5"
        rx="1"
        ry="1"
        fill="none"
        stroke="currentColor"
        strokeWidth="1"
      />
    </svg>
  );
}

function TitleBarSearch() {
  const [query, setQuery] = React.useState("");
  const composingRef = React.useRef(false);
  const openSearchPane = useAppStore((state) => state.openSearchPane);

  const submitSearch = (event: React.KeyboardEvent<HTMLInputElement>) => {
    if (event.key !== "Enter") return;
    if (
      composingRef.current ||
      event.nativeEvent.isComposing ||
      event.keyCode === 229
    ) {
      return;
    }

    const trimmed = query.trim();
    if (!trimmed) return;
    event.preventDefault();
    openSearchPane(trimmed);
    setQuery("");
  };

  return (
    <label className="input input-xs input-bordered relative z-[9999] flex w-[250px] items-center gap-2 border-surface0 bg-base-100">
      <Search className="h-3.5 w-3.5 text-subtext0" />
      <input
        className="grow text-xs"
        value={query}
        placeholder={t("Search... (?query for YQ)")}
        onChange={(event) => setQuery(event.target.value)}
        onCompositionStart={() => {
          composingRef.current = true;
        }}
        onCompositionEnd={() => {
          window.setTimeout(() => {
            composingRef.current = false;
          }, 0);
        }}
        onKeyDown={submitSearch}
      />
    </label>
  );
}
