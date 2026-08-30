import React from "react";
import {
  DragDropContext,
  Draggable,
  Droppable,
  type DropResult,
} from "@hello-pangea/dnd";
import {
  GripVertical,
  Plus,
  Save,
  Trash2,
} from "lucide-react";
import { t } from "../../i18n";
import { useAppStore } from "../../store/appStore";
import type { SidecarEntry, SidecarSettings } from "../../types/app";
import {
  SIDECAR_DEFAULT_WIDTH,
  SIDECAR_MIN_WIDTH,
  isSupportedSidecarUrl,
  normalizeSidecarWidth,
} from "../../domain/sidecar";

const CssEditor = React.lazy(() =>
  import("../../components/common/SqlEditor").then((module) => ({
    default: module.CssEditor,
  })),
);

function EditorFallback() {
  return (
    <div className="min-h-72 w-full rounded-lg border border-surface0 bg-base-200" />
  );
}

export function SidecarSettingsPanel() {
  const snapshot = useAppStore((state) => state.snapshot!);
  const save = useAppStore((state) => state.saveSetting);
  const [settings, setSettings] = React.useState<SidecarDraftSettings>(() =>
    normalizeSidecarSettings(snapshot.settings.sidecars),
  );
  const [selectedId, setSelectedId] = React.useState<string | null>(
    () => settings.entries[0]?.id ?? null,
  );

  React.useEffect(() => {
    const next = normalizeSidecarSettings(snapshot.settings.sidecars);
    setSettings(next);
    setSelectedId((current) =>
      current && next.entries.some((entry) => entry.id === current)
        ? current
        : next.entries[0]?.id ?? null,
    );
  }, [snapshot.settings.sidecars]);

  const selected =
    settings.entries.find((entry) => entry.id === selectedId) ??
    settings.entries[0] ??
    null;
  const items = sidecarListItems(settings);
  const hasInvalidUrl = settings.entries.some(
    (entry) => !isSupportedSidecarUrl(entry.url),
  );

  const addSidecar = () => {
    setSettings((current) => {
      const entry = createSidecarEntry();
      setSelectedId(entry.id);
      return {
        entries: [...current.entries, entry],
        mainViewIndex: 0,
      };
    });
  };

  const removeSidecar = () => {
    if (!selected) return;
    setSettings((current) => {
      const index = current.entries.findIndex(
        (entry) => entry.id === selected.id,
      );
      if (index < 0) return current;
      const entries = current.entries.filter(
        (entry) => entry.id !== selected.id,
      );
      const nextSelected = entries[Math.min(index, entries.length - 1)] ?? null;
      setSelectedId(nextSelected?.id ?? null);
      return normalizeSidecarSettings({ entries, mainViewIndex: 0 });
    });
  };

  const updateSidecar = (patch: Partial<SidecarDraftEntry>) => {
    if (!selected) return;
    setSettings((current) => ({
      ...current,
      entries: current.entries.map((entry) =>
        entry.id === selected.id ? { ...entry, ...patch } : entry,
      ),
    }));
  };

  const moveSidecarItem = (from: number, to: number) => {
    if (from === to) return;
    setSettings((current) => {
      const normalized = normalizeSidecarSettings(current);
      return normalizeSidecarSettings(moveSidecarListItem(normalized, from, to));
    });
  };

  const handleSidecarDragEnd = (result: DropResult) => {
    if (!result.destination) return;
    moveSidecarItem(result.source.index, result.destination.index);
  };

  const persist = () => {
    if (hasInvalidUrl) return;
    void save("sidecars", serializeSidecarSettings(settings));
  };

  return (
    <div className="flex h-full bg-base-100">
      <aside className="w-64 shrink-0 border-r border-surface0 bg-base-300">
        <div className="py-1">
          <div
            className="mx-2 my-1 flex h-10 items-center gap-2 rounded-md border border-blue/50 bg-base px-3 text-left text-sm font-semibold text-text"
          >
            {t("Main View")}
          </div>
          <DragDropContext onDragEnd={handleSidecarDragEnd}>
            <Droppable droppableId="sidecar-settings-list">
              {(provided) => (
                <div
                  ref={provided.innerRef}
                  className="flex flex-col"
                  {...provided.droppableProps}
                >
                  {items.map((item, index) => (
                    <Draggable
                      draggableId={sidecarDraggableId(item.entry)}
                      index={index}
                      key={sidecarDraggableId(item.entry)}
                    >
                      {(provided, snapshot) => (
                        <button
                          ref={provided.innerRef}
                          className={`flex h-10 items-center gap-2 border-b border-surface0 px-2 text-left text-sm ${
                            selected?.id === item.entry.id
                              ? "bg-base text-text"
                              : "text-subtext0 hover:bg-surface0/60 hover:text-text"
                          } ${snapshot.isDragging ? "shadow-lg" : ""}`}
                          onClick={() => {
                            setSelectedId(item.entry.id);
                          }}
                          {...provided.draggableProps}
                        >
                          <span
                            className="grid h-full w-5 shrink-0 cursor-grab place-items-center text-overlay0 active:cursor-grabbing"
                            {...provided.dragHandleProps}
                          >
                            <GripVertical className="h-3.5 w-3.5" />
                          </span>
                          <span className="truncate">{item.entry.name}</span>
                          <span className="ml-auto shrink-0 text-xs text-overlay0">
                            {normalizeSidecarWidth(item.entry.width)}px
                          </span>
                        </button>
                      )}
                    </Draggable>
                  ))}
                  {provided.placeholder}
                </div>
              )}
            </Droppable>
          </DragDropContext>
          <button
            className="flex h-10 items-center gap-2 px-3 text-left text-sm text-text hover:bg-surface0/60"
            onClick={addSidecar}
          >
            <Plus className="h-3.5 w-3.5" />
            {t("Add Sidecar")}
          </button>
          {selected ? (
            <button
              className="flex h-10 items-center gap-2 px-3 text-left text-sm text-red hover:bg-surface0/60"
              onClick={removeSidecar}
            >
              <Trash2 className="h-3.5 w-3.5" />
              {t("Remove Sidecar")}
            </button>
          ) : null}
        </div>
      </aside>
      <section className="flex min-h-0 min-w-0 flex-1 flex-col">
        {selected ? (
          <div className="min-h-0 flex-1 overflow-auto p-6">
            <div className="mb-6">
              <h1 className="text-lg font-semibold">{selected.name}</h1>
              <div className="mt-3 text-sm text-subtext0">
                {sidecarPlacementLabel(settings, selected.id)}
              </div>
            </div>
            <div className="settings-grid timeline-tab-settings-grid">
              <label className="contents">
                <span className="self-center text-sm text-subtext0">
                  {t("Name")}
                </span>
                <input
                  className="input input-bordered input-sm max-w-xs border-surface0 bg-base-200"
                  value={selected.name}
                  onChange={(event) =>
                    updateSidecar({ name: event.target.value })
                  }
                />
              </label>
              <label className="contents">
                <span className="self-center text-sm text-subtext0">URL</span>
                <input
                  className={`input input-bordered input-sm max-w-xl bg-base-200 ${isSupportedSidecarUrl(selected.url) ? "border-surface0" : "border-red"}`}
                  value={selected.url}
                  onChange={(event) =>
                    updateSidecar({ url: event.target.value })
                  }
                />
              </label>
              <label className="contents">
                <span className="self-center text-sm text-subtext0">
                  {t("Width")}
                </span>
                <input
                  className="input input-bordered input-sm w-28 border-surface0 bg-base-200"
                  type="number"
                  min={SIDECAR_MIN_WIDTH}
                  value={selected.width}
                  onChange={(event) =>
                    updateSidecar({
                      width: event.target.value,
                    })
                  }
                />
              </label>
              <div className="contents">
                <span className="self-start pt-1 text-sm text-subtext0">
                  {t("UserStyle")}
                </span>
                <div className="min-w-0 w-full">
                  <label className="mb-2 flex items-center gap-3 text-sm text-text">
                    <input
                      checked={selected.userStyleEnabled}
                      className="toggle toggle-primary toggle-sm"
                      type="checkbox"
                      onChange={(event) =>
                        updateSidecar({
                          userStyleEnabled: event.target.checked,
                        })
                      }
                    />
                    <span>{t("Enable UserStyle")}</span>
                  </label>
                  <p className="mb-3 text-xs leading-relaxed text-yellow">
                    {t(
                      "Applying UserStyle requires JavaScript injection into the Sidecar WebView and can affect the displayed site. Use it only with sites you trust.",
                    )}
                  </p>
                  <React.Suspense fallback={<EditorFallback />}>
                    <CssEditor
                      className="w-full"
                      value={selected.userStyle}
                      onChange={(userStyle) => updateSidecar({ userStyle })}
                    />
                  </React.Suspense>
                </div>
              </div>
            </div>
          </div>
        ) : (
          <div className="grid min-h-0 flex-1 place-items-center text-sm text-subtext0">
            <button className="btn btn-secondary btn-sm" onClick={addSidecar}>
              <Plus className="h-4 w-4" />
              {t("Add Sidecar")}
            </button>
          </div>
        )}
        <div className="flex shrink-0 justify-end gap-2 border-t border-surface0 px-6 py-4">
          {selected ? (
            <button
              className="btn btn-secondary btn-sm"
              onClick={removeSidecar}
            >
              <Trash2 className="h-4 w-4" />
              {t("Delete")}
            </button>
          ) : null}
          <button
            className="btn btn-primary btn-sm"
            disabled={hasInvalidUrl}
            onClick={persist}
          >
            <Save className="h-4 w-4" />
            {t("Save")}
          </button>
        </div>
      </section>
    </div>
  );
}

type SidecarListItem = { entry: SidecarDraftEntry };

type SidecarDraftEntry = Omit<SidecarEntry, "width"> & { width: string };

type SidecarDraftSettings = {
  entries: SidecarDraftEntry[];
  mainViewIndex: number;
};

function normalizeSidecarSettings(
  settings?: SidecarSettings | SidecarDraftSettings,
): SidecarDraftSettings {
  const entries =
    settings?.entries.map((entry) => ({
      ...entry,
      name: entry.name.trim() || "Sidecar",
      url: entry.url.trim(),
      userStyleEnabled: entry.userStyleEnabled ?? false,
      userStyle: entry.userStyle ?? "",
      width:
        typeof entry.width === "string" && entry.width.trim() === ""
          ? ""
          : String(normalizeSidecarWidth(entry.width)),
    })) ?? [];
  return {
    entries,
    mainViewIndex: 0,
  };
}

function sidecarListItems(settings: SidecarDraftSettings): SidecarListItem[] {
  const normalized = normalizeSidecarSettings(settings);
  return normalized.entries.map((entry) => ({ entry }));
}

function sidecarDraggableId(entry: SidecarDraftEntry) {
  return `sidecar-${entry.id}`;
}

function moveSidecarListItem(
  settings: SidecarDraftSettings,
  fromIndex: number,
  toIndex: number,
): SidecarDraftSettings {
  const items = sidecarListItems(settings);
  if (!items[fromIndex] || !items[toIndex]) return settings;
  const next = [...items];
  const [item] = next.splice(fromIndex, 1);
  next.splice(toIndex, 0, item);
  const entries: SidecarDraftEntry[] = [];
  for (const nextItem of next) {
    entries.push(nextItem.entry);
  }
  return { entries, mainViewIndex: 0 };
}

function createSidecarEntry(): SidecarDraftEntry {
  return {
    id:
      typeof crypto !== "undefined" && "randomUUID" in crypto
        ? crypto.randomUUID()
        : `${Date.now()}-${Math.random()}`,
    name: "X",
    url: "https://x.com",
    userStyleEnabled: false,
    userStyle: "",
    width: String(SIDECAR_DEFAULT_WIDTH),
  };
}

function serializeSidecarSettings(
  settings: SidecarDraftSettings,
): SidecarSettings {
  const entries = settings.entries.map((entry) => ({
    ...entry,
    name: entry.name.trim() || "Sidecar",
    url: entry.url.trim(),
    userStyleEnabled: entry.userStyleEnabled ?? false,
    userStyle: entry.userStyle ?? "",
    width: normalizeSidecarWidth(entry.width),
  }));
  return {
    entries,
    mainViewIndex: 0,
  };
}

function sidecarPlacementLabel(
  settings: SidecarSettings | SidecarDraftSettings,
  entryId: string,
) {
  const index = settings.entries.findIndex((entry) => entry.id === entryId);
  if (index < 0) return "";
  return index < settings.mainViewIndex ? t("Left side") : t("Right side");
}
