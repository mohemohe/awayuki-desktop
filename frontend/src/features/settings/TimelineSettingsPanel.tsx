import React from "react";
import {
  Check,
  ChevronDown,
  Copy,
  ExternalLink,
  GripVertical,
  Plus,
  RefreshCw,
  Save,
  Trash2,
} from "lucide-react";
import {
  DragDropContext,
  Draggable,
  Droppable,
  type DropResult,
} from "@hello-pangea/dnd";
import { invokeTypedReadCommand } from "../../api/tauri";
import { t, translateKnownMessage } from "../../i18n";
import { useAppStore } from "../../store/appStore";
import type {
  AccountListSummary,
  AccountSummary,
  ColumnSummary,
} from "../../types/app";
import {
  createTimelineEditorState,
  reduceTimelineEditor,
} from "../../domain/timelineEditor";
import {
  defaultTimelineName,
  displayTimelineName,
  flattenPanes,
  groupColumnsByPane,
  normalizeDisplayFilter,
  timelineTypeSupportsDisplayFilter,
} from "../../utils/columns";
import {
  availableConfigurableTimelineTypesForSessions,
  timelineDescriptor,
  timelineTypeRequiresAccount,
} from "../../domain/timelineDescriptors";
import { hasTopLevelSqlLimit } from "../../utils/sql";
import { copyToClipboard, openExternalUrl } from "../../utils/browser";
import { SelectRow, ToggleRow } from "../../components/common/FormRows";
import {
  notificationSoundLabel,
  paneNotificationSoundValues,
  type PaneNotificationSound,
} from "../../utils/notificationSound";

const SqlEditor = React.lazy(() =>
  import("../../components/common/SqlEditor").then((module) => ({
    default: module.SqlEditor,
  })),
);
const YqEditor = React.lazy(() =>
  import("../../components/common/SqlEditor").then((module) => ({
    default: module.YqEditor,
  })),
);
const KqEditor = React.lazy(() =>
  import("../../components/common/SqlEditor").then((module) => ({
    default: module.KqEditor,
  })),
);

function EditorFallback() {
  return (
    <div className="min-h-72 w-full rounded-lg border border-surface0 bg-base-200" />
  );
}

export const CUSTOM_TIMELINE_SCHEMA = [
  {
    label: "statuses",
    values: [
      "id",
      "server_domain",
      "uri",
      "url",
      "created_at",
      "edited_at",
      "account_id",
      "content",
      "visibility",
      "sensitive",
      "spoiler_text",
      "reblogs_count",
      "favourites_count",
      "replies_count",
      "in_reply_to_id",
      "in_reply_to_account_id",
      "reblog_of_id",
      "language",
      "pinned",
      "favourited",
      "reblogged",
      "muted",
      "bookmarked",
      "poll_json",
      "card_json",
      "application_json",
      "mentions_json",
      "tags_json",
      "emojis_json",
      "media_attachments_json",
      "fetched_at",
      "quote_id",
      "quote_original_url",
    ],
  },
  {
    label: "accounts",
    values: [
      "id",
      "server_domain",
      "username",
      "acct",
      "display_name",
      "note",
      "avatar",
      "avatar_static",
      "header",
      "locked",
      "bot",
      "followers_count",
      "following_count",
      "statuses_count",
      "created_at",
      "fetched_at",
      "fields_json",
      "emojis_json",
    ],
  },
  {
    label: "notifications",
    values: [
      "id",
      "server_domain",
      "account_acct",
      "notification_type",
      "created_at",
      "account_id",
      "status_id",
      "read_at",
      "fetched_at",
    ],
  },
  {
    label: "timeline_entries",
    values: [
      "id",
      "timeline_type",
      "server_domain",
      "status_id",
      "account_acct",
      "position_at",
    ],
  },
  {
    label: "status_tags",
    values: ["status_id", "server_domain", "tag_name"],
  },
  {
    label: "status_viewer_state",
    values: [
      "login_account_acct",
      "status_id",
      "server_domain",
      "favourited",
      "reblogged",
      "muted",
      "bookmarked",
      "pinned",
      "updated_at",
    ],
  },
  {
    label: "tags",
    values: ["name", "server_domain"],
  },
  {
    label: "status_search_icu_content",
    values: [
      "docid",
      "status_id",
      "server_domain",
      "token_text",
      "text_scope_version",
    ],
  },
  {
    label: "status_search_icu_fts",
    values: ["rowid", "token_text"],
  },
  {
    label: "account_search_icu_content",
    values: ["docid", "account_id", "server_domain", "token_text"],
  },
  {
    label: "account_search_icu_fts",
    values: ["rowid", "token_text"],
  },
] as const;

export const CUSTOM_TIMELINE_QUERY_EXAMPLES = [
  {
    label: "Latest statuses",
    sql: `SELECT *
FROM statuses
WHERE visibility = 'public'
ORDER BY created_at DESC, server_domain DESC, id DESC
LIMIT 100`,
  },
  {
    label: "Hashtag search",
    sql: `SELECT s.*
FROM status_tags search_tag
JOIN statuses s
  ON s.id = search_tag.status_id
 AND s.server_domain = search_tag.server_domain
WHERE search_tag.tag_name IN ('hashtag1', 'hashtag2')
ORDER BY s.created_at DESC, s.server_domain DESC, s.id DESC
LIMIT 100`,
  },
  {
    label: "Status full-text search",
    sql: `-- Searches statuses.content and statuses.spoiler_text only.
-- ICU token: "awayuki" -> x61776179756b69
SELECT s.*
FROM status_search_icu_fts
JOIN status_search_icu_content search_status
  ON search_status.docid = status_search_icu_fts.rowid
JOIN statuses s
  ON s.id = search_status.status_id
 AND s.server_domain = search_status.server_domain
WHERE status_search_icu_fts MATCH '"x61776179756b69"*'
ORDER BY s.created_at DESC, s.server_domain DESC, s.id DESC
LIMIT 100`,
  },
  {
    label: "Account full-text search",
    sql: `-- ICU token: "alice" -> x616c696365
SELECT s.*
FROM account_search_icu_fts
JOIN account_search_icu_content search_account
  ON search_account.docid = account_search_icu_fts.rowid
JOIN statuses s
  ON s.account_id = search_account.account_id
 AND s.server_domain = search_account.server_domain
WHERE account_search_icu_fts MATCH '"x616c696365"*'
ORDER BY s.created_at DESC, s.server_domain DESC, s.id DESC
LIMIT 100`,
  },
] as const;

const YQ_REFERENCE = [
  {
    label: "syntax",
    values: [
      "[from <source>] [where] <S-expression>",
      '(contains text "keyword")',
      'where (regex text "pattern")',
    ],
  },
  {
    label: "status variables",
    values: [
      "text",
      "content",
      "raw_content",
      "visibility",
      "language",
      "lang",
      "application",
      "application_name",
      "source",
      "source_app",
      "spoiler_text",
      "cw",
      "sensitive",
      "favourites_count",
      "fav_count",
      "reblogs_count",
      "boost_count",
      "replies_count",
      "bookmarked",
      "favourited",
      "faved",
      "reblogged",
      "boosted",
      "muted",
      "pinned",
      "in_reply_to_id",
      "is_reply",
      "is_reblog",
      "is_boost",
      "has_media",
      "has_poll",
      "has_card",
      "has_cw",
      "server_domain",
      "domain",
    ],
  },
  {
    label: "account variables",
    values: ["user", "username", "acct", "display_name", "bot", "locked"],
  },
  {
    label: "functions",
    values: [
      "and",
      "&",
      "or",
      "|",
      "not",
      "!",
      "equals",
      "eq",
      "=",
      "==",
      "noteq",
      "neq",
      "!=",
      "/=",
      "contains",
      "in",
      "list",
      "quote",
      "+",
      "-",
      "*",
      "/",
      "%",
      "mod",
      "regex",
    ],
  },
] as const;

export const KQ_REFERENCE = [
  {
    label: "syntax",
    values: [
      "from <source>[, <source>...] [where <expression>]",
      "where <expression>",
      'from list:"list-id" where <expression>',
      'from list:"acct/list-id" where <expression>',
      'from search:"keyword" where <expression>',
      "[value, ...]",
      "@alice-smith@sub.example.social",
      '@"alice@example.social:8443"',
      '#"did:plc:abc..."',
    ],
  },
  {
    label: "sources",
    values: [
      "local",
      "all",
      "*",
      "home",
      'home:"acct"',
      "mention",
      "mentions",
      'mentions:"acct"',
      "reply",
      "replies",
      "message",
      "messages",
      'messages:"acct"',
      "dm",
      "dms",
      "direct",
      'list:"list-id"',
      'list:"acct/list-id"',
      'search:"keyword"',
      'find:"keyword"',
      'track:"keyword"',
      'stream:"keyword"',
      'conv:"social.example/status-id"',
      'conversation:"social.example/status-id"',
      'talk:"social.example/status-id"',
      'tree:"social.example/status-id"',
      'user:"acct"',
      "public",
      'public:"acct"',
      "federated",
      "local_public",
      'local_public:"acct"',
      "localpublic",
      'hashtag:"tag"',
      'tag:"tag"',
      "bookmarks",
      'bookmarks:"acct"',
      "bookmarked",
      "favourites",
      'favourites:"acct"',
      "favorites",
      "favs",
    ],
  },
  {
    label: "status variables",
    values: [
      "direct_message",
      "retweet",
      "reblog",
      "boost",
      "renote",
      "has_media",
      "id",
      "uri",
      "url",
      "in_reply_to",
      "to",
      "favs",
      "retweets",
      "favourites_count",
      "reblogs_count",
      "replies_count",
      "text",
      "content",
      "raw_content",
      "via",
      "application",
      "application_name",
      "quote",
      "reply",
      "visibility",
      "is_public",
      "is_unlisted",
      "is_private",
      "is_direct",
      "language",
      "lang",
      "spoiler_text",
      "cw",
      "has_cw",
      "sensitive",
      "edited",
      "edited_at",
      "server_domain",
      "domain",
      "hashtags",
      "tags",
    ],
  },
  {
    label: "account variables",
    values: [
      "user",
      "author",
      "retweeter",
      "reblogger",
      "booster",
      "author.id",
      "author.acct",
      "author.username",
      "author.display_name",
      "author.description",
      "author.note",
      "author.locked",
      "author.protected",
      "author.bot",
      "author.is_bot",
      "author.statuses_count",
      "author.following_count",
      "author.followers_count",
      "author.server_domain",
      "booster.id",
      "booster.acct",
      "booster.username",
      "booster.display_name",
      "booster.description",
      "booster.note",
      "booster.locked",
      "booster.protected",
      "booster.bot",
      "booster.is_bot",
      "booster.statuses_count",
      "booster.following_count",
      "booster.followers_count",
      "booster.server_domain",
    ],
  },
  {
    label: "viewer variables",
    values: [
      "viewer.favourited",
      "viewer.reblogged",
      "viewer.bookmarked",
      "viewer.muted",
      "viewer.pinned",
    ],
  },
  {
    label: "reply & quote variables",
    values: [
      "reply.id",
      "reply.account_id",
      "quote",
      "quote.id",
      "quote.url",
      "quote.text",
      "quote.author.acct",
      "quote.user.acct",
    ],
  },
  {
    label: "media & poll variables",
    values: [
      "media.count",
      "media.types",
      "media.descriptions",
      "has_image",
      "has_video",
      "has_audio",
      "has_poll",
      "poll.id",
      "poll.expired",
      "poll.multiple",
      "poll.votes_count",
      "poll.voters_count",
      "poll.options_count",
      "poll.options",
      "poll.expires_at",
      "has_card",
    ],
  },
  {
    label: "operators",
    values: [
      "!",
      "*",
      "/",
      "+",
      "-",
      "<",
      "<=",
      ">",
      ">=",
      "=",
      "==",
      "!=",
      "&",
      "&&",
      "|",
      "||",
      "contains",
      "->",
      "in",
      "<-",
      "startswith",
      "startwith",
      "endswith",
      "endwith",
      "regex",
      "match",
      "caseful",
    ],
  },
  {
    label: "Awayuki operator extensions",
    values: ["and", "or", "not"],
  },
] as const;

const timelineTypeLabel = (value: string) => {
  const descriptor = timelineDescriptor(value);
  return descriptor
    ? t(descriptor.labelId)
    : t("timeline.unknown", { type: value });
};

export function TimelineSettingsPanel() {
  const snapshot = useAppStore((state) => state.snapshot!);
  const columns = snapshot.columns;
  const saveColumns = useAppStore((state) => state.saveColumns);
  const [editor, dispatchEditor] = React.useReducer(
    reduceTimelineEditor,
    groupColumnsByPane(columns),
    createTimelineEditorState,
  );
  const { panes, selectedPane, selectedTabId } = editor;

  React.useEffect(() => {
    dispatchEditor({ type: "reset", panes: groupColumnsByPane(columns) });
  }, [columns]);

  const pane = panes[selectedPane];
  const selectedTab =
    pane?.tabs.find((tab) => tab.id === selectedTabId) ?? pane?.tabs[0] ?? null;
  const defaultAccountAcct =
    snapshot.activeAcct ?? snapshot.accounts[0]?.acct ?? null;

  const selectPane = (index: number) =>
    dispatchEditor({ type: "selectPane", index });
  const addPane = () => dispatchEditor({ type: "addPane" });
  const removePane = () => dispatchEditor({ type: "removePane" });
  const addTab = () => dispatchEditor({ type: "addTab" });
  const removeTab = () => dispatchEditor({ type: "removeTab" });

  const movePane = (from: number, to: number) => {
    dispatchEditor({ type: "movePane", from, to });
  };

  const moveTab = (from: number, to: number) => {
    dispatchEditor({ type: "moveTab", from, to });
  };

  const handlePaneDragEnd = (result: DropResult) => {
    if (!result.destination) return;
    movePane(result.source.index, result.destination.index);
  };

  const handleTabDragEnd = (result: DropResult) => {
    if (!result.destination) return;
    moveTab(result.source.index, result.destination.index);
  };

  const updateTab = (patch: Partial<ColumnSummary>) => {
    dispatchEditor({ type: "updateTab", patch });
  };
  const updatePane = (
    patch: Partial<
      Pick<
        NonNullable<typeof pane>,
        "desktopNotifications" | "notificationSound"
      >
    >,
  ) => dispatchEditor({ type: "updatePane", patch });

  const persist = () => void saveColumns(flattenPanes(panes));
  const selectedColumnType = selectedTab?.columnType;
  const selectedTimelineDescriptor = selectedColumnType
    ? timelineDescriptor(selectedColumnType)
    : undefined;
  const timelineTypeOptions: readonly string[] = React.useMemo(() => {
    const available = availableConfigurableTimelineTypesForSessions(
      snapshot.accounts.map((account) => account.capabilities),
    );
    return selectedColumnType &&
      !available.some((type) => type === selectedColumnType)
      ? [selectedColumnType, ...available]
      : available;
  }, [selectedColumnType, snapshot.accounts]);
  const isTextParamColumn = selectedTimelineDescriptor?.parameterEditor === "text";
  const textParamLabel =
    selectedColumnType === "search"
      ? t("Search Query")
      : selectedColumnType === "yq"
        ? t("YQ Query")
        : selectedColumnType === "kq"
          ? t("KQ Query")
          : t("Parameter");
  const maxStatusesDisabled =
    selectedTimelineDescriptor?.parameterEditor === "sql" &&
    hasTopLevelSqlLimit(selectedTab.columnParam ?? "");

  return (
    <div className="flex h-full bg-base-100">
      <aside className="w-36 shrink-0 border-r border-surface0 bg-base-300">
        <div className="py-1">
          <DragDropContext onDragEnd={handlePaneDragEnd}>
            <Droppable droppableId="timeline-pane-settings-list">
              {(provided) => (
                <div
                  ref={provided.innerRef}
                  className="flex flex-col"
                  {...provided.droppableProps}
                >
                  {panes.map((item, index) => (
                    <Draggable
                      draggableId={`timeline-pane-${item.paneIndex}`}
                      index={index}
                      key={item.paneIndex}
                    >
                      {(provided, snapshot) => (
                        <button
                          ref={provided.innerRef}
                          className={`flex h-10 items-center gap-2 border-b border-surface0 px-2 text-left text-sm ${
                            selectedPane === index
                              ? "bg-base text-text"
                              : "text-subtext0 hover:bg-surface0/60 hover:text-text"
                          } ${snapshot.isDragging ? "shadow-lg" : ""}`}
                          onClick={() => selectPane(index)}
                          {...provided.draggableProps}
                        >
                          <span
                            className="grid h-full w-5 shrink-0 cursor-grab place-items-center text-overlay0 active:cursor-grabbing"
                            {...provided.dragHandleProps}
                          >
                            <GripVertical className="h-3.5 w-3.5" />
                          </span>
                          <span className="truncate">
                            {t("Pane {index} ({count})", {
                              index: index + 1,
                              count: item.tabs.length,
                            })}
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
            onClick={addPane}
          >
            <Plus className="h-3.5 w-3.5" />
            {t("Add Pane")}
          </button>
          {pane ? (
            <button
              className="flex h-10 items-center gap-2 px-3 text-left text-sm text-red hover:bg-surface0/60"
              onClick={removePane}
            >
              <Trash2 className="h-3.5 w-3.5" />
              {t("Remove Pane")}
            </button>
          ) : null}
        </div>
      </aside>
      <aside className="w-40 shrink-0 border-r border-surface0 bg-base-300">
        <div className="py-1">
          <DragDropContext onDragEnd={handleTabDragEnd}>
            <Droppable droppableId={`timeline-tab-settings-list-${selectedPane}`}>
              {(provided) => (
                <div
                  ref={provided.innerRef}
                  className="flex flex-col"
                  {...provided.droppableProps}
                >
                  {pane?.tabs.map((tab, index) => (
                    <Draggable
                      draggableId={`timeline-tab-${tab.id}`}
                      index={index}
                      key={tab.id}
                    >
                      {(provided, snapshot) => (
                        <button
                          ref={provided.innerRef}
                          className={`flex h-10 items-center gap-2 border-b border-surface0 px-2 text-left text-sm ${
                            selectedTab?.id === tab.id
                              ? "bg-base text-text"
                              : "text-subtext0 hover:bg-surface0/60 hover:text-text"
                          } ${snapshot.isDragging ? "shadow-lg" : ""}`}
                          onClick={() =>
                            dispatchEditor({ type: "selectTab", id: tab.id })
                          }
                          {...provided.draggableProps}
                        >
                          <span
                            className="grid h-full w-5 shrink-0 cursor-grab place-items-center text-overlay0 active:cursor-grabbing"
                            {...provided.dragHandleProps}
                          >
                            <GripVertical className="h-3.5 w-3.5" />
                          </span>
                          <span className="truncate">
                            {displayTimelineName(tab)}
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
          {pane ? (
            <button
              className="flex h-10 items-center gap-2 px-3 text-left text-sm text-subtext0 hover:bg-surface0/60 hover:text-text"
              onClick={addTab}
            >
              <Plus className="h-3.5 w-3.5" />
              {t("Add Tab")}
            </button>
          ) : null}
        </div>
      </aside>
      <section className="flex min-h-0 min-w-0 flex-1 flex-col">
        {selectedTab ? (
          <>
            <div className="min-h-0 flex-1 overflow-auto p-6">
              <div className="mb-6">
                <h1 className="text-lg font-semibold">
                  {displayTimelineName(selectedTab)}
                </h1>
                <div className="mt-3 text-sm text-subtext0">
                  {t("Type: {type}", {
                    type: timelineTypeLabel(selectedTab.columnType),
                  })}
                </div>
              </div>
              <div className="settings-grid timeline-tab-settings-grid">
                <label className="contents">
                  <span className="self-center text-sm text-subtext0">
                    {t("Name")}
                  </span>
                  <input
                    className="input input-bordered input-sm max-w-xs border-surface0 bg-base-200"
                    value={selectedTab.name}
                    onChange={(event) => updateTab({ name: event.target.value })}
                  />
                </label>
                <ToggleRow
                  label={t("Desktop notifications")}
                  checked={pane.desktopNotifications}
                  onChange={(desktopNotifications) =>
                    updatePane({ desktopNotifications })
                  }
                />
                <SelectRow
                  label={t("Notification sound")}
                  value={pane.notificationSound ?? "Inherit"}
                  values={paneNotificationSoundValues}
                  optionLabel={notificationSoundLabel}
                  onChange={(value: PaneNotificationSound) =>
                    updatePane({
                      notificationSound: value === "Inherit" ? null : value,
                    })
                  }
                />
              <SelectRow
                label={t("Type")}
                value={selectedTab.columnType}
                values={timelineTypeOptions}
                optionLabel={timelineTypeLabel}
                onChange={(columnType) =>
                  updateTab({
                    columnType,
                    displayFilter: timelineTypeSupportsDisplayFilter(columnType)
                      ? normalizeDisplayFilter(selectedTab.displayFilter)
                      : undefined,
                    accountAcct: timelineTypeRequiresAccount(columnType)
                      ? (selectedTab.accountAcct ?? defaultAccountAcct)
                      : null,
                  })
                }
              />
              {timelineTypeRequiresAccount(selectedTab.columnType) &&
              selectedTimelineDescriptor?.parameterEditor !== "list" ? (
                <AccountColumnEditor
                  tab={selectedTab}
                  accounts={snapshot.accounts}
                  defaultAcct={defaultAccountAcct}
                  onUpdate={updateTab}
                />
              ) : null}
              {selectedTimelineDescriptor?.parameterEditor === "list" ? (
                <ListColumnEditor
                  tab={selectedTab}
                  accounts={snapshot.accounts}
                  defaultAcct={defaultAccountAcct}
                  onUpdate={updateTab}
                />
              ) : null}
              {selectedTimelineDescriptor?.parameterEditor === "sql" ? (
                <>
                  <div className="contents">
                    <span className="self-start pt-2 text-sm text-subtext0">
                      SQL
                    </span>
                    <React.Suspense fallback={<EditorFallback />}>
                      <SqlEditor
                        className="w-full"
                        value={selectedTab.columnParam ?? ""}
                        onChange={(columnParam) => updateTab({ columnParam })}
                      />
                    </React.Suspense>
                  </div>
                  <IcuTokenConverter />
                  <ReferenceHelp
                    title={t("Schema Reference")}
                    sections={CUSTOM_TIMELINE_SCHEMA}
                  />
                  <SqlQueryExamples />
                </>
              ) : selectedTimelineDescriptor?.parameterEditor === "yq" ? (
                <>
                  <div className="contents">
                    <span className="self-start pt-2 text-sm text-subtext0">
                      {textParamLabel}
                    </span>
                    <React.Suspense fallback={<EditorFallback />}>
                      <YqEditor
                        className="w-full"
                        value={selectedTab.columnParam ?? ""}
                        onChange={(columnParam) => updateTab({ columnParam })}
                      />
                    </React.Suspense>
                  </div>
                  <ReferenceHelp
                    title={t("YQ Reference")}
                    link={{
                      href: "https://github.com/shibafu528/Yukari/wiki/Yukari-Query",
                      label: t("Yukari Query Wiki"),
                    }}
                    sections={YQ_REFERENCE}
                  />
                </>
              ) : selectedTimelineDescriptor?.parameterEditor === "kq" ? (
                <>
                  <div className="contents">
                    <span className="self-start pt-2 text-sm text-subtext0">
                      {textParamLabel}
                    </span>
                    <React.Suspense fallback={<EditorFallback />}>
                      <KqEditor
                        className="w-full"
                        value={selectedTab.columnParam ?? ""}
                        onChange={(columnParam) => updateTab({ columnParam })}
                      />
                    </React.Suspense>
                  </div>
                  <ReferenceHelp
                    title={t("KQ Reference")}
                    link={{
                      href: "https://github.com/mohemohe/awayuki-desktop/blob/main/docs/kq-query-reference.md",
                      label: t("Krile Query Language"),
                    }}
                    sections={KQ_REFERENCE}
                  />
                </>
              ) : isTextParamColumn ? (
                <label className="contents">
                  <span className="self-center text-sm text-subtext0">
                    {textParamLabel}
                  </span>
                  <textarea
                    className="textarea textarea-bordered min-h-24 max-w-xl border-surface0 bg-base-200 text-sm"
                    value={selectedTab.columnParam ?? ""}
                    onChange={(event) =>
                      updateTab({ columnParam: event.target.value })
                    }
                  />
                </label>
              ) : null}
              {timelineTypeSupportsDisplayFilter(selectedTab.columnType) ? (
                <DisplayFilterEditor tab={selectedTab} onUpdate={updateTab} />
              ) : null}
              <label className="contents">
                <span className="self-center text-sm text-subtext0">
                  {t("Max Statuses")}
                </span>
                <input
                  className="input input-bordered input-sm w-28 border-surface0 bg-base-200"
                  type="number"
                  min={1}
                  disabled={maxStatusesDisabled}
                  value={selectedTab.maxStatuses}
                  onChange={(event) =>
                    updateTab({ maxStatuses: Number(event.target.value) || 1 })
                  }
                />
              </label>
            </div>
            </div>
            <div className="flex shrink-0 justify-end gap-2 border-t border-surface0 px-6 py-4">
              <button className="btn btn-secondary btn-sm" onClick={removeTab}>
                <Trash2 className="h-4 w-4" />
                {t("Delete")}
              </button>
              <button className="btn btn-primary btn-sm" onClick={persist}>
                <Save className="h-4 w-4" />
                {t("Save")}
              </button>
            </div>
          </>
        ) : (
          <div className="grid h-full place-items-center text-sm text-subtext0">
            <button className="btn btn-secondary btn-sm" onClick={addTab}>
              <Plus className="h-4 w-4" />
              {t("Add Tab")}
            </button>
          </div>
        )}
      </section>
    </div>
  );
}

function DisplayFilterEditor({
  tab,
  onUpdate,
}: {
  tab: ColumnSummary;
  onUpdate: (patch: Partial<ColumnSummary>) => void;
}) {
  const filter = normalizeDisplayFilter(tab.displayFilter);
  const updateFilter = (patch: Partial<typeof filter>) =>
    onUpdate({ displayFilter: { ...filter, ...patch } });

  return (
    <>
      <ToggleRow
        label={t("Display filter")}
        checked={filter.enabled}
        onChange={(enabled) => updateFilter({ enabled })}
      />
      <ToggleRow
        label={t("Exclude boosts")}
        checked={filter.excludeBoosts}
        disabled={!filter.enabled}
        onChange={(excludeBoosts) => updateFilter({ excludeBoosts })}
      />
      <ToggleRow
        label={t("Exclude media")}
        checked={filter.excludeMedia}
        disabled={!filter.enabled}
        onChange={(excludeMedia) => updateFilter({ excludeMedia })}
      />
      <ToggleRow
        label={t("Include media")}
        checked={filter.includeMedia}
        disabled={!filter.enabled}
        onChange={(includeMedia) => updateFilter({ includeMedia })}
      />
    </>
  );
}

type AccountListFetchState = {
  lists: AccountListSummary[];
  loading: boolean;
  fetched: boolean;
  autoFetched: boolean;
};

type ReferenceSection = {
  readonly label: string;
  readonly values: readonly string[];
};

function ReferenceHelp({
  title,
  link,
  sections,
}: {
  title: string;
  link?: { href: string; label: string };
  sections: readonly ReferenceSection[];
}) {
  return (
    <div className="contents">
      <span aria-hidden="true" />
      <div className="timeline-query-help max-w-3xl rounded-md border border-surface0 bg-base-200/70 p-4 text-sm">
        <div className="mb-3 flex flex-wrap items-center gap-x-3 gap-y-1">
          <div className="font-semibold text-subtext0">{title}</div>
          {link ? (
            <a
              className="inline-flex items-center gap-1 text-xs font-medium text-blue hover:underline"
              href={link.href}
              onClick={(event) => {
                event.preventDefault();
                void openExternalUrl(link.href).catch((error) => {
                  console.error("Failed to open timeline reference link", error);
                });
              }}
              rel="noreferrer"
              target="_blank"
            >
              {link.label}
              <ExternalLink className="h-3 w-3" />
            </a>
          ) : null}
        </div>
        <div className="space-y-4">
          {sections.map((section) => (
            <div key={section.label}>
              <div className="mb-1 text-xs font-semibold uppercase text-overlay1">
                {translateKnownMessage(section.label)}
              </div>
              <div className="timeline-query-help-values">
                {section.values.map((value) => (
                  <code
                    className="timeline-query-help-value"
                    key={`${section.label}-${value}`}
                  >
                    {value}
                  </code>
                ))}
              </div>
            </div>
          ))}
        </div>
      </div>
    </div>
  );
}

function SqlQueryExamples() {
  return (
    <div className="contents">
      <span aria-hidden="true" />
      <div className="timeline-query-help min-w-0 max-w-3xl rounded-md border border-surface0 bg-base-200/70 p-4 text-sm">
        <div className="mb-3 font-semibold text-subtext0">
          {t("Query Examples")}
        </div>
        <div className="space-y-4">
          {CUSTOM_TIMELINE_QUERY_EXAMPLES.map((example) => (
            <div key={example.label}>
              <div className="mb-1 text-xs font-semibold uppercase text-overlay1">
                {t(example.label)}
              </div>
              <pre className="max-w-full overflow-x-auto rounded-md bg-base-300 p-3 text-xs leading-relaxed text-text">
                <code>{example.sql}</code>
              </pre>
            </div>
          ))}
        </div>
      </div>
    </div>
  );
}

export function IcuTokenConverter() {
  const [token, setToken] = React.useState("");
  const [matchExpression, setMatchExpression] = React.useState("");
  const [converting, setConverting] = React.useState(false);
  const [conversionFailed, setConversionFailed] = React.useState(false);
  const [copied, setCopied] = React.useState(false);

  React.useEffect(() => {
    if (!token) {
      setMatchExpression("");
      setConverting(false);
      setConversionFailed(false);
      return;
    }

    let active = true;
    setConverting(true);
    setConversionFailed(false);
    void invokeTypedReadCommand("icu_match_expression", {
      request: { term: token },
    })
      .then((expression) => {
        if (active) setMatchExpression(expression ?? "");
      })
      .catch((error) => {
        if (!active) return;
        setMatchExpression("");
        setConversionFailed(true);
        console.error("Failed to build ICU FTS MATCH expression", error);
      })
      .finally(() => {
        if (active) setConverting(false);
      });

    return () => {
      active = false;
    };
  }, [token]);

  const copyMatchExpression = () => {
    if (!matchExpression) return;
    void copyToClipboard(matchExpression)
      .then(() => setCopied(true))
      .catch((error) => {
        console.error("Failed to copy ICU FTS MATCH expression", error);
      });
  };

  return (
    <div className="contents">
      <span aria-hidden="true" />
      <div className="timeline-query-help min-w-0 max-w-3xl rounded-md border border-surface0 bg-base-200/70 p-4 text-sm">
        <div className="mb-3 font-semibold text-subtext0">
          {t("ICU MATCH Expression Converter")}
        </div>
        <div className="grid gap-3 sm:grid-cols-2">
          <label className="grid min-w-0 gap-1">
            <span className="text-xs font-semibold uppercase text-overlay1">
              {t("Search term")}
            </span>
            <input
              className="input input-bordered input-sm min-w-0 border-surface0 bg-base-300 font-mono text-sm"
              value={token}
              onChange={(event) => {
                setToken(event.target.value);
                setCopied(false);
              }}
              placeholder="awayuki"
            />
          </label>
          <div className="grid min-w-0 gap-1">
            <span className="text-xs font-semibold uppercase text-overlay1">
              {t("MATCH expression")}
            </span>
            <div className="flex min-w-0 gap-2">
              <output
                aria-label={t("MATCH expression")}
                className="input input-bordered input-sm flex min-w-0 flex-1 items-center overflow-x-auto whitespace-nowrap border-surface0 bg-base-300 font-mono text-sm"
              >
                {converting
                  ? t("Converting...")
                  : conversionFailed
                    ? t("Conversion failed")
                    : matchExpression}
              </output>
              <button
                className="btn btn-secondary btn-sm shrink-0"
                type="button"
                disabled={converting || !matchExpression}
                onClick={copyMatchExpression}
              >
                {copied ? (
                  <Check className="h-4 w-4" />
                ) : (
                  <Copy className="h-4 w-4" />
                )}
                {copied ? t("Copied") : t("Copy")}
              </button>
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}

function AccountColumnEditor({
  tab,
  accounts,
  defaultAcct,
  onUpdate,
}: {
  tab: ColumnSummary;
  accounts: AccountSummary[];
  defaultAcct?: string | null;
  onUpdate: (patch: Partial<ColumnSummary>) => void;
}) {
  const accountAcct = tab.accountAcct ?? defaultAcct ?? accounts[0]?.acct ?? "";
  return (
    <>
      <span className="self-center text-sm text-subtext0">{t("Account")}</span>
      <span className="contents">
        <span className="relative inline-flex max-w-xs">
          <select
            className="select select-bordered select-sm h-8 min-h-8 w-full appearance-none border-surface0 bg-base-200 bg-none pr-8 text-sm"
            value={accountAcct}
            onChange={(event) =>
              onUpdate({ accountAcct: event.target.value || null })
            }
            disabled={accounts.length === 0}
          >
            {accounts.map((account) => (
              <option key={account.acct} value={account.acct}>
                {account.displayName || account.acct} (@{account.acct})
              </option>
            ))}
          </select>
          <ChevronDown className="pointer-events-none absolute right-3 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-subtext0" />
        </span>
      </span>
    </>
  );
}

function ListColumnEditor({
  tab,
  accounts,
  defaultAcct,
  onUpdate,
}: {
  tab: ColumnSummary;
  accounts: AccountSummary[];
  defaultAcct?: string | null;
  onUpdate: (patch: Partial<ColumnSummary>) => void;
}) {
  const [listsByAccount, setListsByAccount] = React.useState<
    Record<string, AccountListFetchState>
  >({});
  const accountAcct = tab.accountAcct ?? defaultAcct ?? accounts[0]?.acct ?? "";
  const listState = accountAcct ? listsByAccount[accountAcct] : undefined;
  const lists = listState?.lists ?? [];
  const selectedList = lists.find((list) => list.id === tab.columnParam);

  const fetchLists = React.useCallback(
    async (acct: string, autoFetched = false) => {
      if (!acct) return;
      setListsByAccount((current) => ({
        ...current,
        [acct]: {
          lists: current[acct]?.lists ?? [],
          fetched: current[acct]?.fetched ?? false,
          autoFetched: autoFetched || (current[acct]?.autoFetched ?? false),
          loading: true,
        },
      }));
      try {
        const lists = await invokeTypedReadCommand(
          "account_lists",
          {
            request: { acct },
          },
        );
        setListsByAccount((current) => ({
          ...current,
          [acct]: {
            lists,
            fetched: true,
            autoFetched: autoFetched || (current[acct]?.autoFetched ?? false),
            loading: false,
          },
        }));
      } catch (error) {
        useAppStore.setState({ error: String(error) });
        setListsByAccount((current) => ({
          ...current,
          [acct]: {
            lists: current[acct]?.lists ?? [],
            fetched: current[acct]?.fetched ?? false,
            autoFetched: autoFetched || (current[acct]?.autoFetched ?? false),
            loading: false,
          },
        }));
      }
    },
    [],
  );

  React.useEffect(() => {
    if (!accountAcct) return;
    if (listState?.loading || listState?.fetched || listState?.autoFetched)
      return;
    void fetchLists(accountAcct, true);
  }, [
    accountAcct,
    fetchLists,
    listState?.autoFetched,
    listState?.fetched,
    listState?.loading,
  ]);

  const updateAccount = (acct: string) => {
    const resetName =
      tab.name === defaultTimelineName("list") ||
      tab.name === selectedList?.title ||
      tab.name.startsWith("List:");
    onUpdate({
      accountAcct: acct,
      columnParam: null,
      ...(resetName ? { name: defaultTimelineName("list") } : {}),
    });
  };

  const updateList = (listId: string) => {
    const list = lists.find((item) => item.id === listId);
    const rename =
      !tab.columnParam ||
      tab.name === defaultTimelineName("list") ||
      tab.name === selectedList?.title ||
      tab.name.startsWith("List:");
    onUpdate({
      columnParam: listId,
      ...(rename && list ? { name: list.title } : {}),
    });
  };

  const hasCurrentUnfetchedList =
    Boolean(tab.columnParam) &&
    !lists.some((list) => list.id === tab.columnParam);
  const emptyListLabel = listState?.loading
    ? t("Loading...")
    : listState?.fetched
      ? lists.length > 0
        ? t("Select list")
        : t("No lists")
      : t("Fetch lists");

  return (
    <>
      <span className="self-center text-sm text-subtext0">{t("Account")}</span>
      <span className="contents">
        <span className="relative inline-flex max-w-xs">
          <select
            className="select select-bordered select-sm h-8 min-h-8 w-full appearance-none border-surface0 bg-base-200 bg-none pr-8 text-sm"
            value={accountAcct}
            onChange={(event) => updateAccount(event.target.value)}
            disabled={accounts.length === 0}
          >
            {accounts.map((account) => (
              <option key={account.acct} value={account.acct}>
                {account.displayName || account.acct} (@{account.acct})
              </option>
            ))}
          </select>
          <ChevronDown className="pointer-events-none absolute right-3 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-subtext0" />
        </span>
      </span>
      <span className="self-center text-sm text-subtext0">{t("List")}</span>
      <span className="contents">
        <span className="flex min-w-0 max-w-xl items-center gap-2">
          <span className="relative inline-flex min-w-0 flex-1">
            <select
              className="select select-bordered select-sm h-8 min-h-8 w-full appearance-none border-surface0 bg-base-200 bg-none pr-8 text-sm"
              value={tab.columnParam ?? ""}
              onChange={(event) => updateList(event.target.value)}
              disabled={
                listState?.loading || (!listState?.fetched && !tab.columnParam)
              }
            >
              <option value="" disabled>
                {emptyListLabel}
              </option>
              {hasCurrentUnfetchedList ? (
                <option value={tab.columnParam ?? ""}>
                  {t("Current: {value}", { value: tab.columnParam ?? "" })}
                </option>
              ) : null}
              {lists.map((list) => (
                <option key={list.id} value={list.id}>
                  {list.title}
                </option>
              ))}
            </select>
            <ChevronDown className="pointer-events-none absolute right-3 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-subtext0" />
          </span>
          <button
            type="button"
            className="btn btn-secondary btn-sm h-8 min-h-8 shrink-0 px-3 text-sm font-normal"
            onClick={() => void fetchLists(accountAcct)}
            disabled={!accountAcct || listState?.loading}
            title={t("Fetch lists")}
          >
            <RefreshCw
              className={`h-4 w-4 ${listState?.loading ? "animate-spin" : ""}`}
            />
            {t("Fetch")}
          </button>
        </span>
      </span>
    </>
  );
}
