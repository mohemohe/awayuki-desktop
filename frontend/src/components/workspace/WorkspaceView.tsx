import React from "react";
import { Search } from "lucide-react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { ComposeArea } from "../compose/ComposeArea";
import { StatusBar } from "../status/StatusBar";
import { TimelineArea } from "../timeline/TimelineArea";
import { hasTauriRuntime } from "../../api/tauri";
import { useAppStore } from "../../store/appStore";
import { getClientPlatform } from "../../utils/browser";
import { groupColumnsByPane } from "../../utils/columns";
import { t } from "../../i18n";

export function WorkspaceView() {
  const snapshot = useAppStore((state) => state.snapshot);
  const activeTabs = useAppStore((state) => state.activeTabs);
  const dynamicColumns = useAppStore((state) => state.dynamicColumns);
  if (!snapshot) return null;

  const panes = groupColumnsByPane([...snapshot.columns, ...dynamicColumns]);

  return (
    <div className="flex h-screen min-h-0 flex-col overflow-hidden">
      <CustomTitleBar />
      <ComposeArea />
      <TimelineArea panes={panes} activeTabs={activeTabs} />
      <StatusBar />
    </div>
  );
}

function CustomTitleBar() {
  const platform = getClientPlatform();
  const isMac = platform === "macos";
  const titlePaddingClass = isMac ? "pl-20" : "pl-0";
  const headerPaddingClass = isMac ? "px-2" : "pl-2 pr-0";

  return (
    <header
      className={`relative grid h-8 shrink-0 grid-cols-[1fr_auto_1fr] items-center border-b border-surface0 bg-crust ${headerPaddingClass} text-xs text-subtext0`}
      data-tauri-drag-region
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
        {isMac ? <TitleBarSearch /> : <WindowControls />}
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
    <label className="input input-xs input-bordered flex w-[250px] items-center gap-2 border-surface0 bg-base-100">
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
