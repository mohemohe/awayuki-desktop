import React from "react";
import {
  ArrowDown,
  ArrowUp,
  ChevronDown,
  ChevronLeft,
  ExternalLink,
  GripVertical,
  Plus,
  RefreshCw,
  Save,
  Trash2,
} from "lucide-react";
import { invokeCommand } from "../../api/tauri";
import { useAppStore, type SettingsSection } from "../../store/appStore";
import type {
  AccountListSummary,
  AccountRateLimitSummary,
  AccountSummary,
  AccountSourceColor,
  AppearanceSettings,
  ConfirmationSettings,
  DbSummary,
  DebugSettings,
  NotificationMutedAccountSummary,
  PerformanceSettings,
  PresetVisibilitySettings,
  ColumnSummary,
  PaneGroup,
} from "../../types/app";
import { ACCOUNT_SOURCE_COLORS } from "../../constants/accountSourceColors";
import {
  createColumn,
  defaultTimelineName,
  displayTimelineName,
  flattenPanes,
  groupColumnsByPane,
  normalizeDisplayFilter,
  timelineTypeSupportsDisplayFilter,
  timelineTypes,
} from "../../utils/columns";
import { getClientPlatform } from "../../utils/browser";
import { formatDuration, formatTime } from "../../utils/format";
import { hasTopLevelSqlLimit } from "../../utils/sql";
import { presetVisibilityValues } from "../../utils/visibility";
import { Avatar } from "../common/Avatar";
import { Metric } from "../common/Metric";
import { SelectRow, ToggleRow } from "../common/FormRows";
import { SqlEditor, YqEditor } from "../common/SqlEditor";
import { appLocale, t } from "../../i18n";

const BLUESKY_FETCH_INTERVAL_OPTIONS = [
  { seconds: 10, label: "10s", labelJa: "10 秒" },
  { seconds: 15, label: "15s", labelJa: "15 秒" },
  { seconds: 30, label: "30s", labelJa: "30 秒" },
  { seconds: 60, label: "1m", labelJa: "1 分" },
  { seconds: 120, label: "2m", labelJa: "2 分" },
  { seconds: 300, label: "5m", labelJa: "5 分" },
] as const;

const optionLabel = (value: string) => t(value);
const timelineTypeLabel = (value: string) => t(defaultTimelineName(value));
const translationEngineLabel = (
  value: ConfirmationSettings["translation_engine"],
) =>
  t(
    value === "FoundationModel"
      ? "Apple Intelligence Foundation Model"
      : "Apple Translation Framework",
  );
const blueskyFetchIntervalLabel = (
  option: (typeof BLUESKY_FETCH_INTERVAL_OPTIONS)[number],
) => (appLocale === "ja" ? option.labelJa : option.label);

const CUSTOM_TIMELINE_SCHEMA = [
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

export function SettingsView() {
  const selectedSettings = useAppStore((state) => state.selectedSettings);
  const sections: SettingsSection[] = [
    "Account",
    "Timeline",
    "Notification",
    "Behavior",
    "Appearance",
    "Performance",
    "Database",
    "Debug",
    "About",
  ];

  return (
    <div className="flex h-screen flex-col bg-base-100">
      <div
        className="h-8 shrink-0 border-b border-surface0 bg-base-300"
        data-tauri-drag-region
      />
      <div className="flex min-h-0 flex-1">
        <aside className="w-40 shrink-0 border-r border-surface0 bg-base-300">
          <button
            className="btn btn-secondary btn-sm m-2 h-8 min-h-8 px-4 text-sm font-normal"
            onClick={() => useAppStore.setState({ settingsOpen: false })}
          >
            <ChevronLeft className="h-4 w-4" />
            {t("Back")}
          </button>
          <nav className="mt-2 flex flex-col">
            {sections.map((section) => (
              <button
                key={section}
                className={`h-10 px-3 text-left text-sm font-normal ${selectedSettings === section ? "bg-surface0 text-text" : "text-subtext0 hover:bg-surface0/60 hover:text-text"}`}
                onClick={() =>
                  useAppStore.setState({ selectedSettings: section })
                }
              >
                {t(section)}
              </button>
            ))}
          </nav>
        </aside>
        <section
          className={`min-w-0 flex-1 overflow-y-auto bg-base ${selectedSettings === "Timeline" ? "" : "px-6 py-7"}`}
        >
          {selectedSettings === "Timeline" ? null : (
            <div className="settings-content">
              <h1 className="mb-5 text-lg font-normal text-text">
                {t(selectedSettings)}
              </h1>
              <SettingsPanel section={selectedSettings} />
            </div>
          )}
          {selectedSettings === "Timeline" ? (
            <SettingsPanel section={selectedSettings} />
          ) : null}
        </section>
      </div>
    </div>
  );
}

function SettingsPanel({ section }: { section: SettingsSection }) {
  const snapshot = useAppStore((state) => state.snapshot);
  if (!snapshot) return null;
  if (section === "Account") return <AccountSettings />;
  if (section === "Appearance") return <AppearanceSettingsPanel />;
  if (section === "Behavior") return <BehaviorSettingsPanel />;
  if (section === "Performance") return <PerformanceSettingsPanel />;
  if (section === "Notification") return <NotificationSettingsPanel />;
  if (section === "Timeline") return <TimelineSettingsPanel />;
  if (section === "Database") return <DatabaseSettingsPanel />;
  if (section === "Debug") return <DebugSettingsPanel />;
  return <AboutPanel />;
}

function AccountSettings() {
  const snapshot = useAppStore((state) => state.snapshot);
  const switchAccount = useAppStore((state) => state.switchAccount);
  const logoutAccount = useAppStore((state) => state.logoutAccount);
  const refreshAccounts = useAppStore((state) => state.refreshAccounts);
  const save = useAppStore((state) => state.saveSetting);
  const hasBlueskyAccount =
    snapshot?.accounts.some((account) => account.serverKind === "bluesky") ??
    false;
  React.useEffect(() => {
    if (!hasBlueskyAccount) return;
    void refreshAccounts();
    const timer = window.setInterval(() => {
      void refreshAccounts();
    }, 30_000);
    return () => window.clearInterval(timer);
  }, [hasBlueskyAccount, refreshAccounts]);
  if (!snapshot) return null;
  const updateSourceColor = (acct: string, color: AccountSourceColor) => {
    const next = { ...snapshot.settings.accountSourceColors };
    if (color === "Transparent") delete next[acct];
    else next[acct] = color;
    void save("account_source_colors", next);
  };
  const updateBlueskyFetchInterval = (acct: string, seconds: number) => {
    const existing = snapshot.settings.blueskyFetch.intervals_by_acct ?? {};
    const fallback =
      snapshot.settings.blueskyFetch.interval_seconds ??
      BLUESKY_FETCH_INTERVAL_OPTIONS[2].seconds;
    const intervalsByAcct = Object.fromEntries(
      snapshot.accounts
        .filter((account) => account.serverKind === "bluesky")
        .map((account) => [account.acct, existing[account.acct] ?? fallback]),
    );
    intervalsByAcct[acct] = seconds;
    void save("bluesky_fetch", { intervals_by_acct: intervalsByAcct });
  };
  const blueskyFetchIntervalFor = (acct: string) => {
    return (
      snapshot.settings.blueskyFetch.intervals_by_acct?.[acct] ??
      snapshot.settings.blueskyFetch.interval_seconds ??
      BLUESKY_FETCH_INTERVAL_OPTIONS[2].seconds
    );
  };
  return (
    <div className="space-y-4">
      {snapshot.accounts.map((account) => (
        <div
          key={account.acct}
          className={`rounded-md border bg-base-200 px-4 py-4 ${account.acct === snapshot.activeAcct ? "border-blue" : "border-base-200"}`}
        >
          <div className="flex items-start gap-3">
            <Avatar
              src={account.avatar}
              label={account.displayName || account.acct}
              size="xl"
            />
            <div className="min-w-0 flex-1">
              <div className="flex min-w-0 items-center gap-2">
                <div className="truncate text-sm font-semibold text-text">
                  {account.displayName || account.acct}
                </div>
                {account.acct === snapshot.activeAcct ? (
                  <span className="rounded bg-blue px-2 py-0.5 text-xs text-crust">
                    {t("Active")}
                  </span>
                ) : null}
              </div>
              <div className="mt-1 truncate text-sm text-subtext0">
                @{account.acct}
              </div>
            </div>
            <div className="flex shrink-0 gap-2">
              {account.acct !== snapshot.activeAcct ? (
                <button
                  className="btn btn-secondary btn-sm h-8 min-h-8 px-4 text-sm font-normal"
                  onClick={() => void switchAccount(account.acct)}
                >
                  {t("Activate")}
                </button>
              ) : null}
              <button
                className="btn btn-error btn-sm h-8 min-h-8 px-4 text-sm font-normal"
                onClick={() => void logoutAccount(account.acct)}
              >
                {t("Logout")}
              </button>
            </div>
          </div>
          {account.serverKind === "bluesky" && account.rateLimit ? (
            <AccountRateLimit rateLimit={account.rateLimit} />
          ) : null}
          {account.serverKind === "bluesky" ? (
            <div className="mt-4 border-t border-surface0 pt-3">
              <div className="flex flex-wrap items-center justify-between gap-3">
                <div className="text-xs font-semibold text-subtext0">
                  {t("Bluesky fetch interval")}
                </div>
                <BlueskyFetchIntervalDropdown
                  value={blueskyFetchIntervalFor(account.acct)}
                  onChange={(seconds) =>
                    updateBlueskyFetchInterval(account.acct, seconds)
                  }
                />
              </div>
            </div>
          ) : null}
          <div className="mt-4 border-t border-surface0 pt-3">
            <div className="mb-2 text-xs font-semibold text-subtext0">
              {t("Timeline source color")}
            </div>
            <AccountSourceColorPicker
              value={
                snapshot.settings.accountSourceColors[account.acct] ??
                "Transparent"
              }
              onChange={(color) => updateSourceColor(account.acct, color)}
            />
          </div>
        </div>
      ))}
      <button
        className="btn btn-secondary btn-sm h-8 min-h-8 px-4 text-sm font-normal"
        onClick={() => useAppStore.setState({ loginOpen: true })}
      >
        <Plus className="h-4 w-4" />
        {t("Add Account")}
      </button>
    </div>
  );
}

function BlueskyFetchIntervalDropdown({
  value,
  onChange,
}: {
  value: number;
  onChange: (seconds: number) => void;
}) {
  const selected =
    BLUESKY_FETCH_INTERVAL_OPTIONS.find((option) => option.seconds === value) ??
    BLUESKY_FETCH_INTERVAL_OPTIONS[2];
  return (
    <div className="dropdown dropdown-end">
      <button
        type="button"
        tabIndex={0}
        className="btn btn-secondary btn-sm h-8 min-h-8 min-w-24 justify-between px-3 text-sm font-normal"
      >
        {blueskyFetchIntervalLabel(selected)}
        <ChevronDown className="h-3.5 w-3.5" />
      </button>
      <ul
        tabIndex={0}
        className="menu dropdown-content z-10 mt-1 w-32 rounded-md border border-surface0 bg-base-200 p-1 shadow-lg"
      >
        {BLUESKY_FETCH_INTERVAL_OPTIONS.map((option) => (
          <li key={option.seconds}>
            <button
              type="button"
              className={option.seconds === selected.seconds ? "active" : ""}
              onClick={() => onChange(option.seconds)}
            >
              {blueskyFetchIntervalLabel(option)}
            </button>
          </li>
        ))}
      </ul>
    </div>
  );
}

function AccountSourceColorPicker({
  value,
  onChange,
}: {
  value: AccountSourceColor;
  onChange: (color: AccountSourceColor) => void;
}) {
  return (
    <div className="flex min-w-0 flex-wrap items-center gap-2">
      {ACCOUNT_SOURCE_COLORS.map((color) => {
        const selected = color.value === value;
        const transparent = color.value === "Transparent";
        return (
          <button
            key={color.value}
            type="button"
            className={`account-source-color-swatch h-6 w-6 shrink-0 border ${
              selected
                ? "border-blue ring-2 ring-blue/60"
                : "border-surface1 hover:border-blue"
            } ${transparent ? "account-source-color-transparent" : ""}`}
            style={transparent ? undefined : { backgroundColor: color.hex }}
            aria-label={t(color.label)}
            title={t(color.label)}
            onClick={() => onChange(color.value)}
          />
        );
      })}
    </div>
  );
}

function AccountRateLimit({
  rateLimit,
}: {
  rateLimit: AccountRateLimitSummary;
}) {
  return (
    <div className="mt-4 border-t border-surface0 pt-3 text-sm">
      <div className="mb-2 text-xs font-semibold text-subtext0">
        {t("API Rate Limit")}
      </div>
      <div className="text-text">
        {t("{remaining} / {limit} remaining ({used} used)", {
          remaining: rateLimit.remaining,
          limit: rateLimit.limit,
          used: rateLimit.used,
        })}
      </div>
      <div className="mt-2 h-1 rounded bg-surface1">
        <div
          className="h-full rounded bg-blue"
          style={{
            width: `${Math.min(100, Math.max(0, rateLimit.usedFraction * 100))}%`,
          }}
        />
      </div>
      <div className="mt-2 text-xs text-overlay0">
        {t("Resets in {reset} · Updated {updated} ago{policy}", {
          reset: formatDuration(rateLimit.resetInSeconds),
          updated: formatDuration(rateLimit.observedAgoSeconds),
          policy: rateLimit.policy
            ? t(" · Policy: {policy}", { policy: rateLimit.policy })
            : "",
        })}
      </div>
    </div>
  );
}

function AppearanceSettingsPanel() {
  const settings = useAppStore((state) => state.snapshot!.settings.appearance);
  const save = useAppStore((state) => state.saveSetting);
  const update = (patch: Partial<AppearanceSettings>) =>
    void save("appearance", { ...settings, ...patch });
  return (
    <div className="settings-grid">
      <SelectRow
        label={t("Avatar shape")}
        value={settings.avatar_shape}
        values={["Square", "Circle", "Rounded"]}
        optionLabel={optionLabel}
        onChange={(avatar_shape) => update({ avatar_shape })}
      />
      <SelectRow
        label={t("Font size")}
        value={settings.font_size}
        values={["Small", "Medium", "Large"]}
        optionLabel={optionLabel}
        onChange={(font_size) => update({ font_size })}
      />
      <SelectRow
        label={t("CW behavior")}
        value={settings.cw_behavior}
        values={["Hide", "AlwaysExpand"]}
        optionLabel={optionLabel}
        onChange={(cw_behavior) => update({ cw_behavior })}
      />
      <SelectRow
        label={t("NSFW behavior")}
        value={settings.nsfw_behavior}
        values={["Hide", "AlwaysShow"]}
        optionLabel={optionLabel}
        onChange={(nsfw_behavior) => update({ nsfw_behavior })}
      />
      <SelectRow
        label={t("Display mode")}
        value={settings.display_mode}
        values={["StarryEyes", "Mystique"]}
        optionLabel={optionLabel}
        onChange={(display_mode) => update({ display_mode })}
      />
    </div>
  );
}

function BehaviorSettingsPanel() {
  const settings = useAppStore(
    (state) => state.snapshot!.settings.confirmation,
  );
  const presetVisibility = useAppStore(
    (state) => state.snapshot!.settings.presetVisibility,
  );
  const save = useAppStore((state) => state.saveSetting);
  const translationSupported = getClientPlatform() === "macos";
  const update = (patch: Partial<ConfirmationSettings>) =>
    void save("confirmation", { ...settings, ...patch });
  const savePresetVisibility = (entries: PresetVisibilitySettings["entries"]) =>
    void save("preset_visibility", { entries });
  const updatePresetEntry = (
    index: number,
    patch: Partial<PresetVisibilitySettings["entries"][number]>,
  ) => {
    savePresetVisibility(
      presetVisibility.entries.map((entry, entryIndex) =>
        entryIndex === index ? { ...entry, ...patch } : entry,
      ),
    );
  };
  const movePresetEntry = (index: number, direction: -1 | 1) => {
    const nextIndex = index + direction;
    if (nextIndex < 0 || nextIndex >= presetVisibility.entries.length) return;
    const entries = [...presetVisibility.entries];
    [entries[index], entries[nextIndex]] = [entries[nextIndex], entries[index]];
    savePresetVisibility(entries);
  };
  const removePresetEntry = (index: number) =>
    savePresetVisibility(
      presetVisibility.entries.filter((_, entryIndex) => entryIndex !== index),
    );
  return (
    <div className="flex max-w-7xl flex-col gap-8">
      <div className="settings-grid">
        <ToggleRow
          label={t("Confirm boost")}
          checked={settings.confirm_boost}
          onChange={(confirm_boost) => update({ confirm_boost })}
        />
        <ToggleRow
          label={t("Confirm favorite")}
          checked={settings.confirm_favourite}
          onChange={(confirm_favourite) => update({ confirm_favourite })}
        />
        <ToggleRow
          label={t("Confirm follow")}
          checked={settings.confirm_follow}
          onChange={(confirm_follow) => update({ confirm_follow })}
        />
        <ToggleRow
          label={t("Confirm unfollow")}
          checked={settings.confirm_unfollow}
          onChange={(confirm_unfollow) => update({ confirm_unfollow })}
        />
        <ToggleRow
          label={t("Jumbomoji")}
          checked={settings.jumbomoji_enabled ?? false}
          onChange={(jumbomoji_enabled) => update({ jumbomoji_enabled })}
        />
        <ToggleRow
          label={t("Translate posts")}
          checked={settings.translate_enabled}
          disabled={!translationSupported}
          onChange={(translate_enabled) =>
            update({
              translate_enabled,
              auto_translate_enabled: translate_enabled
                ? settings.auto_translate_enabled
                : false,
            })
          }
        />
        <ToggleRow
          label={t("Auto translate posts")}
          checked={
            settings.translate_enabled && settings.auto_translate_enabled
          }
          disabled={!translationSupported || !settings.translate_enabled}
          onChange={(auto_translate_enabled) =>
            update({ auto_translate_enabled })
          }
        />
        <SelectRow
          label={t("Translation engine")}
          value={settings.translation_engine ?? "TranslationFramework"}
          values={["TranslationFramework", "FoundationModel"]}
          optionLabel={translationEngineLabel}
          disabled={!translationSupported}
          onChange={(translation_engine) => update({ translation_engine })}
        />
        {!translationSupported ? (
          <p className="col-span-2 text-xs text-warning">
            {t("Translation is only supported on macOS.")}
          </p>
        ) : null}
        <SelectRow
          label={t("Media source")}
          value={settings.media_source}
          values={["Local", "Remote"]}
          optionLabel={optionLabel}
          onChange={(media_source) => update({ media_source })}
        />
      </div>

      <section className="flex flex-col gap-3">
        <div>
          <h2 className="text-sm font-normal text-text">
            {t("Preset visibility")}
          </h2>
          <p className="mt-2 text-xs text-subtext0">
            {t(
              "Automatically switch visibility when the post text contains a keyword. The first matching preset is applied.",
            )}
          </p>
        </div>
        <div className="flex flex-col gap-2">
          {presetVisibility.entries.map((entry, index) => (
            <div
              key={index}
              className="preset-visibility-row"
              aria-label={t("Preset visibility")}
            >
              <input
                className="input input-bordered input-sm min-w-0 border-surface0 bg-base-200 text-sm"
                value={entry.keyword}
                placeholder={t("Keyword")}
                aria-label={t("Keyword")}
                onChange={(event) =>
                  updatePresetEntry(index, { keyword: event.target.value })
                }
              />
              <span className="relative inline-flex min-w-36">
                <select
                  className="select select-bordered select-sm h-8 min-h-8 w-full appearance-none border-surface0 bg-base-200 bg-none pr-8 text-sm"
                  value={entry.visibility}
                  aria-label={t("Visibility")}
                  onChange={(event) =>
                    updatePresetEntry(index, {
                      visibility: event.target
                        .value as PresetVisibilitySettings["entries"][number]["visibility"],
                    })
                  }
                >
                  {presetVisibilityValues.map((value) => (
                    <option key={value} value={value}>
                      {t(value)}
                    </option>
                  ))}
                </select>
                <ChevronDown className="pointer-events-none absolute right-3 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-subtext0" />
              </span>
              <button
                type="button"
                className="btn btn-ghost btn-xs btn-square"
                title={t("Move preset up")}
                disabled={index === 0}
                onClick={() => movePresetEntry(index, -1)}
              >
                <ArrowUp className="h-3.5 w-3.5" />
              </button>
              <button
                type="button"
                className="btn btn-ghost btn-xs btn-square"
                title={t("Move preset down")}
                disabled={index === presetVisibility.entries.length - 1}
                onClick={() => movePresetEntry(index, 1)}
              >
                <ArrowDown className="h-3.5 w-3.5" />
              </button>
              <button
                type="button"
                className="btn btn-ghost btn-xs btn-square text-red hover:text-red"
                title={t("Remove preset")}
                onClick={() => removePresetEntry(index)}
              >
                <Trash2 className="h-3.5 w-3.5" />
              </button>
            </div>
          ))}
          <button
            type="button"
            className="btn btn-secondary btn-sm w-fit px-4 font-normal"
            onClick={() =>
              savePresetVisibility([
                ...presetVisibility.entries,
                { keyword: "", visibility: "Unlisted" },
              ])
            }
          >
            <Plus className="h-4 w-4" />
            {t("Add preset")}
          </button>
        </div>
      </section>
    </div>
  );
}

function PerformanceSettingsPanel() {
  const settings = useAppStore((state) => state.snapshot!.settings.performance);
  const save = useAppStore((state) => state.saveSetting);
  const update = (patch: Partial<PerformanceSettings>) =>
    void save("performance", { ...settings, ...patch });
  return (
    <div className="settings-grid">
      <SelectRow
        label={t("Mention suggestions")}
        value={settings.mention_source}
        values={["Server", "SQLite"]}
        optionLabel={optionLabel}
        onChange={(mention_source) => update({ mention_source })}
      />
      <SelectRow
        label={t("Hashtag suggestions")}
        value={settings.hashtag_source}
        values={["Server", "SQLite"]}
        optionLabel={optionLabel}
        onChange={(hashtag_source) => update({ hashtag_source })}
      />
      <SelectRow
        label={t("Timeline renderer")}
        value={settings.timeline_renderer}
        values={["List", "VirtualList"]}
        optionLabel={optionLabel}
        onChange={(timeline_renderer) => update({ timeline_renderer })}
      />
    </div>
  );
}

function NotificationSettingsPanel() {
  const [mutedAccounts, setMutedAccounts] = React.useState<
    NotificationMutedAccountSummary[]
  >([]);
  const [loading, setLoading] = React.useState(true);
  const [updating, setUpdating] = React.useState<Record<string, boolean>>({});

  const loadMutedAccounts = React.useCallback(async () => {
    setLoading(true);
    try {
      const accounts = await invokeCommand<NotificationMutedAccountSummary[]>(
        "notification_muted_accounts",
      );
      setMutedAccounts(accounts);
    } catch (error) {
      useAppStore.setState({ error: String(error) });
    } finally {
      setLoading(false);
    }
  }, []);

  React.useEffect(() => {
    void loadMutedAccounts();
  }, [loadMutedAccounts]);

  const unmute = async (account: NotificationMutedAccountSummary) => {
    const key = notificationMuteKey(account);
    setUpdating((current) => ({ ...current, [key]: true }));
    try {
      await invokeCommand<boolean>("set_account_notification_mute", {
        request: {
          accountId: account.accountId,
          serverDomain: account.serverDomain,
          muted: false,
        },
      });
      setMutedAccounts((current) =>
        current.filter((item) => notificationMuteKey(item) !== key),
      );
    } catch (error) {
      useAppStore.setState({ error: String(error) });
    } finally {
      setUpdating((current) => ({ ...current, [key]: false }));
    }
  };

  return (
    <div className="space-y-4 text-sm">
      <div className="flex items-center justify-between gap-3">
        <div className="text-subtext0">
          {t("Desktop notifications from these users are muted.")}
        </div>
        <button
          className="btn btn-secondary btn-sm h-8 min-h-8 px-3 text-sm font-normal"
          onClick={() => void loadMutedAccounts()}
          disabled={loading}
        >
          <RefreshCw className={`h-4 w-4 ${loading ? "animate-spin" : ""}`} />
          {t("Refresh")}
        </button>
      </div>
      <div className="overflow-hidden rounded-md border border-surface0 bg-base-200">
        {loading && mutedAccounts.length === 0 ? (
          <div className="px-4 py-6 text-center text-subtext0">
            {t("Loading...")}
          </div>
        ) : mutedAccounts.length === 0 ? (
          <div className="px-4 py-6 text-center text-subtext0">
            {t("No muted users.")}
          </div>
        ) : (
          mutedAccounts.map((account) => {
            const key = notificationMuteKey(account);
            return (
              <div
                key={key}
                className="flex min-w-0 items-center gap-3 border-b border-surface0 px-4 py-3 last:border-b-0"
              >
                <Avatar
                  src={account.avatar}
                  label={account.displayName || account.acct}
                  size="lg"
                />
                <div className="min-w-0 flex-1">
                  <div className="truncate font-semibold text-text">
                    {account.displayName || account.acct}
                  </div>
                  <div className="mt-0.5 truncate text-subtext0">
                    @{account.acct.replace(/^@/, "")}
                  </div>
                  <div className="mt-1 text-xs text-overlay0">
                    {account.serverDomain} · {t("Muted")}{" "}
                    {formatTime(account.updatedAt)}
                  </div>
                </div>
                <button
                  className="btn btn-secondary btn-sm h-8 min-h-8 shrink-0 px-4 text-sm font-normal"
                  disabled={updating[key]}
                  onClick={() => void unmute(account)}
                >
                  {t("Unmute")}
                </button>
              </div>
            );
          })
        )}
      </div>
    </div>
  );
}

function notificationMuteKey(account: NotificationMutedAccountSummary) {
  return `${account.serverDomain}:${account.accountId}`;
}

function TimelineSettingsPanel() {
  const snapshot = useAppStore((state) => state.snapshot!);
  const columns = snapshot.columns;
  const saveColumns = useAppStore((state) => state.saveColumns);
  const [panes, setPanes] = React.useState<PaneGroup[]>(() =>
    groupColumnsByPane(columns),
  );
  const [selectedPane, setSelectedPane] = React.useState(0);
  const [selectedTabId, setSelectedTabId] = React.useState<string | null>(
    () => groupColumnsByPane(columns)[0]?.tabs[0]?.id ?? null,
  );
  const [draggingPane, setDraggingPane] = React.useState<number | null>(null);
  const [draggingTab, setDraggingTab] = React.useState<number | null>(null);

  React.useEffect(() => {
    const grouped = groupColumnsByPane(columns);
    setPanes(grouped);
    setSelectedPane(0);
    setSelectedTabId(grouped[0]?.tabs[0]?.id ?? null);
  }, [columns]);

  const pane = panes[selectedPane];
  const selectedTab =
    pane?.tabs.find((tab) => tab.id === selectedTabId) ?? pane?.tabs[0] ?? null;
  const defaultAccountAcct =
    snapshot.activeAcct ?? snapshot.accounts[0]?.acct ?? null;

  const selectPane = (index: number) => {
    setSelectedPane(index);
    setSelectedTabId(panes[index]?.tabs[0]?.id ?? null);
  };

  const addPane = () => {
    setPanes((current) => {
      const nextPane: PaneGroup = {
        paneIndex: current.length,
        tabs: [createColumn(current.length, 0)],
      };
      const next = [...current, nextPane];
      setSelectedPane(next.length - 1);
      setSelectedTabId(nextPane.tabs[0].id);
      return next;
    });
  };

  const removePane = () => {
    setPanes((current) => {
      if (!current[selectedPane]) return current;
      const next = current
        .filter((_, index) => index !== selectedPane)
        .map((item, index) => ({ ...item, paneIndex: index }));
      const nextPaneIndex = Math.max(
        0,
        Math.min(selectedPane, next.length - 1),
      );
      setSelectedPane(nextPaneIndex);
      setSelectedTabId(next[nextPaneIndex]?.tabs[0]?.id ?? null);
      return next;
    });
  };

  const addTab = () => {
    setPanes((current) =>
      current.map((item, index) => {
        if (index !== selectedPane) return item;
        const tab = createColumn(index, item.tabs.length);
        setSelectedTabId(tab.id);
        return { ...item, tabs: [...item.tabs, tab] };
      }),
    );
  };

  const removeTab = () => {
    if (!selectedTab) return;
    setPanes((current) =>
      current.map((item, index) => {
        if (index !== selectedPane) return item;
        const tabs = item.tabs
          .filter((tab) => tab.id !== selectedTab.id)
          .map((tab, position) => ({ ...tab, position }));
        setSelectedTabId(tabs[0]?.id ?? null);
        return { ...item, tabs };
      }),
    );
  };

  const movePane = (from: number, to: number) => {
    if (from === to) return;
    setPanes((current) => {
      if (!current[from] || !current[to]) return current;
      const next = [...current];
      const [item] = next.splice(from, 1);
      next.splice(to, 0, item);
      const normalized = next.map((paneItem, index) => ({
        ...paneItem,
        paneIndex: index,
      }));
      setSelectedPane(to);
      setSelectedTabId(normalized[to]?.tabs[0]?.id ?? null);
      return normalized;
    });
  };

  const moveTab = (from: number, to: number) => {
    if (from === to) return;
    setPanes((current) =>
      current.map((item, index) => {
        if (index !== selectedPane || !item.tabs[from] || !item.tabs[to])
          return item;
        const tabs = [...item.tabs];
        const [tab] = tabs.splice(from, 1);
        tabs.splice(to, 0, tab);
        setSelectedTabId(tab.id);
        return {
          ...item,
          tabs: tabs.map((itemTab, position) => ({ ...itemTab, position })),
        };
      }),
    );
  };

  const updateTab = (patch: Partial<ColumnSummary>) => {
    if (!selectedTab) return;
    setPanes((current) =>
      current.map((item, index) => {
        if (index !== selectedPane) return item;
        return {
          ...item,
          tabs: item.tabs.map((tab) => {
            if (tab.id !== selectedTab.id) return tab;
            const hasColumnParamPatch = Object.prototype.hasOwnProperty.call(
              patch,
              "columnParam",
            );
            const nextType = patch.columnType ?? tab.columnType;
            const nextName =
              patch.columnType &&
              tab.name === defaultTimelineName(tab.columnType)
                ? defaultTimelineName(nextType)
                : tab.name;
            return {
              ...tab,
              name: nextName,
              ...patch,
              columnParam:
                patch.columnType &&
                !["hashtag", "list", "custom", "search", "yq"].includes(
                  nextType,
                )
                  ? null
                  : hasColumnParamPatch
                    ? patch.columnParam
                    : tab.columnParam,
            };
          }),
        };
      }),
    );
  };

  const persist = () => void saveColumns(flattenPanes(panes));
  const selectedColumnType = selectedTab?.columnType;
  const isTextParamColumn = selectedColumnType
    ? ["hashtag", "search", "yq"].includes(selectedColumnType)
    : false;
  const textParamLabel =
    selectedColumnType === "search"
      ? t("Search Query")
      : selectedColumnType === "yq"
        ? t("YQ Query")
        : t("Parameter");
  const maxStatusesDisabled =
    selectedTab?.columnType === "custom" &&
    hasTopLevelSqlLimit(selectedTab.columnParam ?? "");

  return (
    <div className="flex h-full min-h-[620px] bg-base-100">
      <aside className="w-36 shrink-0 border-r border-surface0 bg-base-300">
        <div className="flex flex-col py-1">
          {panes.map((item, index) => (
            <button
              key={item.paneIndex}
              draggable
              className={`flex h-10 items-center gap-2 border-b border-surface0 px-2 text-left text-sm ${selectedPane === index ? "bg-base text-text" : "text-subtext0 hover:bg-surface0/60 hover:text-text"}`}
              onClick={() => selectPane(index)}
              onDragStart={() => setDraggingPane(index)}
              onDragOver={(event) => event.preventDefault()}
              onDrop={() => {
                if (draggingPane !== null) movePane(draggingPane, index);
                setDraggingPane(null);
              }}
              onDragEnd={() => setDraggingPane(null)}
            >
              <GripVertical className="h-3.5 w-3.5 shrink-0 text-overlay0" />
              <span className="truncate">
                {t("Pane {index} ({count})", {
                  index: index + 1,
                  count: item.tabs.length,
                })}
              </span>
            </button>
          ))}
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
        <div className="flex flex-col py-1">
          {pane?.tabs.map((tab) => (
            <button
              key={tab.id}
              draggable
              className={`flex h-10 items-center gap-2 border-b border-surface0 px-2 text-left text-sm ${selectedTab?.id === tab.id ? "bg-base text-text" : "text-subtext0 hover:bg-surface0/60 hover:text-text"}`}
              onClick={() => setSelectedTabId(tab.id)}
              onDragStart={() =>
                setDraggingTab(
                  pane.tabs.findIndex((item) => item.id === tab.id),
                )
              }
              onDragOver={(event) => event.preventDefault()}
              onDrop={() => {
                const to = pane.tabs.findIndex((item) => item.id === tab.id);
                if (draggingTab !== null && to >= 0) moveTab(draggingTab, to);
                setDraggingTab(null);
              }}
              onDragEnd={() => setDraggingTab(null)}
            >
              <GripVertical className="h-3.5 w-3.5 shrink-0 text-overlay0" />
              <span className="truncate">{displayTimelineName(tab)}</span>
            </button>
          ))}
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
      <section className="min-w-0 flex-1 p-6">
        {selectedTab ? (
          <div className="flex h-full flex-col">
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
              <SelectRow
                label={t("Type")}
                value={selectedTab.columnType}
                values={timelineTypes}
                optionLabel={timelineTypeLabel}
                onChange={(columnType) =>
                  updateTab({
                    columnType,
                    displayFilter: timelineTypeSupportsDisplayFilter(columnType)
                      ? normalizeDisplayFilter(selectedTab.displayFilter)
                      : undefined,
                    ...(columnType === "list" && !selectedTab.accountAcct
                      ? { accountAcct: defaultAccountAcct }
                      : {}),
                  })
                }
              />
              {selectedTab.columnType === "list" ? (
                <ListColumnEditor
                  tab={selectedTab}
                  accounts={snapshot.accounts}
                  defaultAcct={defaultAccountAcct}
                  onUpdate={updateTab}
                />
              ) : null}
              {selectedTab.columnType === "custom" ? (
                <>
                  <div className="contents">
                    <span className="self-start pt-2 text-sm text-subtext0">
                      SQL
                    </span>
                    <SqlEditor
                      className="w-full"
                      value={selectedTab.columnParam ?? ""}
                      onChange={(columnParam) => updateTab({ columnParam })}
                    />
                  </div>
                  <ReferenceHelp
                    title={t("Schema Reference")}
                    sections={CUSTOM_TIMELINE_SCHEMA}
                  />
                </>
              ) : selectedTab.columnType === "yq" ? (
                <>
                  <div className="contents">
                    <span className="self-start pt-2 text-sm text-subtext0">
                      {textParamLabel}
                    </span>
                    <YqEditor
                      className="w-full"
                      value={selectedTab.columnParam ?? ""}
                      onChange={(columnParam) => updateTab({ columnParam })}
                    />
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
            <div className="mt-auto flex justify-end gap-2">
              <button className="btn btn-secondary btn-sm" onClick={removeTab}>
                <Trash2 className="h-4 w-4" />
                {t("Delete")}
              </button>
              <button className="btn btn-primary btn-sm" onClick={persist}>
                <Save className="h-4 w-4" />
                {t("Save")}
              </button>
            </div>
          </div>
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
                {t(section.label)}
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
        const lists = await invokeCommand<AccountListSummary[]>(
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

function DatabaseSettingsPanel() {
  const database = useAppStore((state) => state.snapshot!.database);
  const refresh = useAppStore((state) => state.loadSnapshot);
  const runDbCommand = async (
    command: "vacuum_database" | "clear_status_cache",
  ) => {
    await invokeCommand<DbSummary>(command);
    await refresh();
  };
  return (
    <div className="space-y-5 text-sm">
      <div className="grid max-w-4xl grid-cols-4 border border-surface0 bg-base-200">
        <Metric
          label={t("Statuses")}
          value={database.statusCount.toLocaleString()}
        />
        <Metric
          label={t("Last 24h")}
          value={database.recentStatusCount.toLocaleString()}
        />
        <Metric
          label={t("Accounts")}
          value={database.accountCount.toLocaleString()}
        />
        <Metric label={t("Size")} value={database.size} />
      </div>
      <div className="text-sm text-subtext0">{database.path}</div>
      <div className="flex gap-2">
        <button
          className="btn btn-secondary btn-sm h-8 min-h-8 px-4 text-sm font-normal"
          onClick={() => void runDbCommand("vacuum_database")}
        >
          {t("Vacuum")}
        </button>
        <button
          className="btn btn-error btn-sm h-8 min-h-8 px-4 text-sm font-normal"
          onClick={() => void runDbCommand("clear_status_cache")}
        >
          {t("Clear Status Cache")}
        </button>
      </div>
    </div>
  );
}

function DebugSettingsPanel() {
  const settings = useAppStore((state) => state.snapshot!.settings.debug);
  const save = useAppStore((state) => state.saveSetting);
  const update = (patch: Partial<DebugSettings>) =>
    void save("debug", { ...settings, ...patch });
  return (
    <div className="settings-grid">
      <ToggleRow
        label={t("File logging")}
        checked={settings.logging_enabled}
        onChange={(logging_enabled) => update({ logging_enabled })}
      />
      <SelectRow
        label={t("Log level")}
        value={settings.log_level}
        values={["Error", "Warn", "Info", "Debug", "Trace"]}
        optionLabel={optionLabel}
        onChange={(log_level) => update({ log_level })}
      />
      <button
        className="btn btn-secondary btn-sm h-8 min-h-8 justify-self-start px-4 text-sm font-normal"
        onClick={() => void invokeCommand("open_log_file")}
      >
        {t("Open Log")}
      </button>
    </div>
  );
}

function AboutPanel() {
  const snapshot = useAppStore((state) => state.snapshot!);
  return (
    <div className="space-y-2 text-sm text-text">
      <div className="font-semibold">Awayuki {snapshot.version}</div>
      <div className="text-subtext0">Tauri / React / Vite / DaisyUI</div>
    </div>
  );
}
