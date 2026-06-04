import React from "react";
import { Home, RefreshCw, Search } from "lucide-react";
import { LogicalPosition, LogicalSize } from "@tauri-apps/api/dpi";
import { Webview } from "@tauri-apps/api/webview";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { ComposeArea } from "../compose/ComposeArea";
import { StatusBar } from "../status/StatusBar";
import { TimelineArea } from "../timeline/TimelineArea";
import { hasTauriRuntime, invokeCommand } from "../../api/tauri";
import { useAppStore } from "../../store/appStore";
import type { SidecarEntry, SidecarSettings } from "../../types/app";
import { getClientPlatform } from "../../utils/browser";
import { groupColumnsByPane } from "../../utils/columns";
import { t } from "../../i18n";

const SIDECAR_MIN_WIDTH = 160;
const SIDECAR_DEFAULT_WIDTH = 500;

export function WorkspaceView() {
  const snapshot = useAppStore((state) => state.snapshot);
  const activeTabs = useAppStore((state) => state.activeTabs);
  const dynamicColumns = useAppStore((state) => state.dynamicColumns);
  const sidecarsVisible = useAppStore((state) => state.mediaPreview == null);
  if (!snapshot) return null;

  const panes = groupColumnsByPane([...snapshot.columns, ...dynamicColumns]);
  const sidecars = normalizeSidecarSettings(snapshot.settings.sidecars);
  const leftSidecars = sidecars.entries.slice(0, sidecars.mainViewIndex);
  const rightSidecars = sidecars.entries.slice(sidecars.mainViewIndex);

  return (
    <div className="flex h-screen min-h-0 flex-col overflow-hidden">
      <CustomTitleBar />
      <ComposeArea />
      <div className="flex min-h-0 flex-1 overflow-hidden">
        <SidecarRegion sidecars={leftSidecars} visible={sidecarsVisible} />
        <TimelineArea panes={panes} activeTabs={activeTabs} />
        <SidecarRegion sidecars={rightSidecars} visible={sidecarsVisible} />
      </div>
      <StatusBar />
    </div>
  );
}

function SidecarRegion({
  sidecars,
  visible,
}: {
  sidecars: SidecarEntry[];
  visible: boolean;
}) {
  const refs = React.useRef<Record<string, HTMLDivElement | null>>({});
  const webviews = React.useRef<Record<string, SidecarWebviewState>>({});
  const updateRequested = React.useRef(false);
  const visibleRef = React.useRef(visible);
  const [errors, setErrors] = React.useState<Record<string, string>>({});
  visibleRef.current = visible;

  const syncWebviews = React.useCallback(async () => {
    updateRequested.current = false;
    const currentIds = new Set(sidecars.map((sidecar) => sidecar.id));
    for (const [id, state] of Object.entries(webviews.current)) {
      if (!currentIds.has(id)) {
        try {
          await state.webview.close();
        } catch (error) {
          console.warn("Failed to close sidecar webview", id, error);
        }
        delete webviews.current[id];
        setErrors((current) => clearSidecarError(current, id));
      }
    }

    if (!hasTauriRuntime()) return;
    if (!visible) {
      await Promise.all(
        Object.entries(webviews.current).map(async ([id, state]) => {
          if (state.status !== "ready") return;
          try {
            await state.webview.hide();
          } catch (error) {
            console.warn("Failed to hide sidecar webview", id, error);
          }
        }),
      );
      return;
    }

    const appWindow = getCurrentWindow();
    for (const sidecar of sidecars) {
      const element = refs.current[sidecar.id];
      if (!element) continue;
      const rect = element.getBoundingClientRect();
      if (rect.width <= 0 || rect.height <= 0) continue;

      const label = sidecarWebviewLabel(sidecar.id);
      let state = webviews.current[sidecar.id];
      if (state && state.url !== sidecar.url) {
        try {
          await state.webview.close();
        } catch (error) {
          console.warn("Failed to recreate sidecar webview", sidecar.id, error);
        }
        delete webviews.current[sidecar.id];
        setErrors((current) => clearSidecarError(current, sidecar.id));
        state = undefined;
      }

      if (!state) {
        const webview = new Webview(appWindow, label, {
          url: sidecar.url,
          x: rect.left,
          y: rect.top,
          width: rect.width,
          height: rect.height,
        });
        state = { webview, url: sidecar.url, status: "pending" };
        webviews.current[sidecar.id] = state;

        void webview.once("tauri://created", () => {
          const currentState = webviews.current[sidecar.id];
          if (!currentState || currentState.webview !== webview) return;
          currentState.status = "ready";
          setErrors((current) => clearSidecarError(current, sidecar.id));
          const currentElement = refs.current[sidecar.id];
          if (!currentElement) return;
          const currentRect = currentElement.getBoundingClientRect();
          if (visibleRef.current) {
            void webview.show();
          } else {
            void webview.hide();
          }
          void webview.setPosition(
            new LogicalPosition(currentRect.left, currentRect.top),
          );
          void webview.setSize(
            new LogicalSize(currentRect.width, currentRect.height),
          );
        });
        void webview.once<unknown>("tauri://error", (event) => {
          const currentState = webviews.current[sidecar.id];
          if (!currentState || currentState.webview !== webview) return;
          currentState.status = "failed";
          const message = formatSidecarError(event.payload);
          setErrors((current) => ({
            ...current,
            [sidecar.id]: message,
          }));
          console.warn("Failed to create sidecar webview", sidecar.id, message);
        });
      } else if (state.status === "ready") {
        await state.webview.show();
        await state.webview.setPosition(
          new LogicalPosition(rect.left, rect.top),
        );
        await state.webview.setSize(new LogicalSize(rect.width, rect.height));
      }
    }
  }, [sidecars, visible]);

  const requestSync = React.useCallback(() => {
    if (updateRequested.current) return;
    updateRequested.current = true;
    window.requestAnimationFrame(() => void syncWebviews());
  }, [syncWebviews]);

  const controlSidecar = React.useCallback(
    async (sidecar: SidecarEntry, action: "home" | "reload") => {
      try {
        if (action === "home") {
          await invokeCommand("navigate_sidecar_webview", {
            sidecarId: sidecar.id,
            url: sidecar.url,
          });
        } else {
          await invokeCommand("reload_sidecar_webview", {
            sidecarId: sidecar.id,
          });
        }
        setErrors((current) => clearSidecarError(current, sidecar.id));
      } catch (error) {
        const message = formatSidecarError(error);
        setErrors((current) => ({
          ...current,
          [sidecar.id]: message,
        }));
        console.warn("Failed to control sidecar webview", sidecar.id, message);
      }
    },
    [],
  );

  React.useLayoutEffect(() => {
    requestSync();
  }, [requestSync]);

  React.useEffect(() => {
    requestSync();
    window.addEventListener("resize", requestSync);
    return () => window.removeEventListener("resize", requestSync);
  }, [requestSync]);

  React.useEffect(() => {
    const resizeObserver = new ResizeObserver(() => requestSync());
    for (const sidecar of sidecars) {
      const element = refs.current[sidecar.id];
      if (element) resizeObserver.observe(element);
    }
    return () => resizeObserver.disconnect();
  }, [requestSync, sidecars]);

  React.useEffect(
    () => () => {
      for (const [id, state] of Object.entries(webviews.current)) {
        void state.webview.close().catch((error) => {
          console.warn("Failed to close sidecar webview", id, error);
        });
      }
      webviews.current = {};
    },
    [],
  );

  if (sidecars.length === 0) return null;

  return (
    <div className="flex h-full shrink-0 overflow-hidden bg-base-100">
      {sidecars.map((sidecar) => (
        <div
          aria-label={sidecar.name}
          className="flex h-full shrink-0 flex-col border-r border-surface0 bg-base-100"
          key={sidecar.id}
          style={{ width: `${Math.max(SIDECAR_MIN_WIDTH, sidecar.width)}px` }}
        >
          <div className="flex h-8 shrink-0 items-stretch border-b border-surface0 bg-base-300">
            <div
              className="flex min-w-0 flex-1 items-center px-3 text-sm text-text"
              title={sidecar.name}
            >
              <span className="block truncate">{sidecar.name}</span>
            </div>
            <div className="flex shrink-0 items-center gap-1 px-1">
              <button
                aria-label={t("Return to sidecar URL")}
                className="btn btn-ghost btn-xs"
                onClick={() => void controlSidecar(sidecar, "home")}
                title={t("Return to sidecar URL")}
              >
                <Home className="h-3.5 w-3.5" />
              </button>
              <button
                aria-label={t("Reload sidecar")}
                className="btn btn-ghost btn-xs"
                onClick={() => void controlSidecar(sidecar, "reload")}
                title={t("Reload sidecar")}
              >
                <RefreshCw className="h-3.5 w-3.5" />
              </button>
            </div>
          </div>
          <div
            className="min-h-0 flex-1 bg-base-100"
            ref={(element) => {
              refs.current[sidecar.id] = element;
            }}
          >
            {errors[sidecar.id] ? (
              <div className="grid h-full place-items-center px-3 text-center text-xs leading-relaxed text-red">
                {errors[sidecar.id]}
              </div>
            ) : null}
          </div>
        </div>
      ))}
    </div>
  );
}

type SidecarWebviewState = {
  webview: Webview;
  url: string;
  status: "pending" | "ready" | "failed";
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

function normalizeSidecarSettings(settings?: SidecarSettings): SidecarSettings {
  const entries =
    settings?.entries
      .filter((entry) => isSupportedSidecarUrl(entry.url))
      .map((entry) => ({
        ...entry,
        width: normalizeSidecarWidth(entry.width),
      })) ?? [];
  return {
    entries,
    mainViewIndex: Math.max(
      0,
      Math.min(Number(settings?.mainViewIndex) || 0, entries.length),
    ),
  };
}

function normalizeSidecarWidth(width: number) {
  const parsed = Number(width);
  if (!Number.isFinite(parsed) || parsed <= 0) return SIDECAR_DEFAULT_WIDTH;
  return Math.max(SIDECAR_MIN_WIDTH, Math.floor(parsed));
}

function sidecarWebviewLabel(id: string) {
  return `sidecar-${id}`.replace(/[^a-zA-Z0-9-/:_]/g, "_");
}

function isSupportedSidecarUrl(url: string) {
  return url.startsWith("https://") || url.startsWith("http://");
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
