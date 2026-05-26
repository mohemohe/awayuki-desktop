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

  return (
    <header
      className="relative grid h-8 shrink-0 grid-cols-[1fr_auto_1fr] items-center border-b border-surface0 bg-crust px-2 text-xs text-subtext0"
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
  const runWindowAction = React.useCallback(
    async (action: "minimize" | "toggleMaximize" | "close") => {
      if (!hasTauriRuntime()) return;
      const appWindow = getCurrentWindow();
      try {
        if (action === "minimize") {
          await appWindow.minimize();
        } else if (action === "toggleMaximize") {
          await appWindow.toggleMaximize();
        } else {
          await appWindow.close();
        }
      } catch (error) {
        console.warn("Window action failed", action, error);
      }
    },
    [],
  );

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
          width="24"
          height="24"
          viewBox="0 0 24 24"
        >
          <path fill="currentColor" d="M19 13H5v-2h14z" />
        </svg>
      </button>
      <button
        id="titlebar-maximize"
        type="button"
        className="grid w-11 place-items-center text-subtext0 hover:bg-surface0 hover:text-text"
        aria-label="maximize"
        title="maximize"
        onClick={() => void runWindowAction("toggleMaximize")}
      >
        <svg
          xmlns="http://www.w3.org/2000/svg"
          width="24"
          height="24"
          viewBox="0 0 24 24"
        >
          <path fill="currentColor" d="M4 4h16v16H4zm2 4v10h12V8z" />
        </svg>
      </button>
      <button
        id="titlebar-close"
        type="button"
        className="grid w-11 place-items-center text-subtext0 hover:bg-red hover:text-crust"
        aria-label="close"
        title="close"
        onClick={() => void runWindowAction("close")}
      >
        <svg
          xmlns="http://www.w3.org/2000/svg"
          width="24"
          height="24"
          viewBox="0 0 24 24"
        >
          <path
            fill="currentColor"
            d="M13.46 12L19 17.54V19h-1.46L12 13.46L6.46 19H5v-1.46L10.54 12L5 6.46V5h1.46L12 10.54L17.54 5H19v1.46z"
          />
        </svg>
      </button>
    </div>
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
