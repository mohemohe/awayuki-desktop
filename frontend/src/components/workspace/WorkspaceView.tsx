import React from "react";
import { Search } from "lucide-react";
import { ComposeArea } from "../compose/ComposeArea";
import { StatusBar } from "../status/StatusBar";
import { TimelineArea } from "../timeline/TimelineArea";
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

  return (
    <header
      className="relative grid h-8 shrink-0 grid-cols-[1fr_auto_1fr] items-center border-b border-surface0 bg-crust px-2 text-xs text-subtext0"
      data-tauri-drag-region
    >
      <div className="flex items-center gap-2 pl-20" data-tauri-drag-region>
        <span className="font-semibold text-text" data-tauri-drag-region>
          Awayuki
        </span>
      </div>
      <div className="flex justify-center" data-tauri-drag-region>
        {isMac ? null : <TitleBarSearch />}
      </div>
      <div className="flex justify-end" data-tauri-drag-region>
        {isMac ? <TitleBarSearch /> : null}
      </div>
    </header>
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
