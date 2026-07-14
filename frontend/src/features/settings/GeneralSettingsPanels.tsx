import React from "react";
import {
  ArrowDown,
  ArrowUp,
  ChevronDown,
  Plus,
  RefreshCw,
  Trash2,
} from "lucide-react";
import {
  invokeTypedCommand,
  invokeTypedReadCommand,
} from "../../api/tauri";
import { isResponseLossError } from "../../api/ipcErrors";
import { ACCOUNT_SOURCE_COLORS } from "../../constants/accountSourceColors";
import { appLocale, t, translateKnownMessage } from "../../i18n";
import { useAppStore } from "../../store/appStore";
import type {
  AccountRateLimitSummary,
  AccountSourceColor,
  AppearanceSettings,
  ConfirmationSettings,
  NotificationMutedAccountSummary,
  PerformanceSettings,
  PresetVisibilitySettings,
} from "../../types/app";
import { getClientPlatform } from "../../utils/browser";
import { formatDuration, formatTime } from "../../utils/format";
import { presetVisibilityValues } from "../../utils/visibility";
import { Avatar } from "../../components/common/Avatar";
import { SelectRow, ToggleRow } from "../../components/common/FormRows";

const BLUESKY_FETCH_INTERVAL_OPTIONS = [
  { seconds: 10, label: "10s", labelJa: "10 \u79d2" },
  { seconds: 15, label: "15s", labelJa: "15 \u79d2" },
  { seconds: 30, label: "30s", labelJa: "30 \u79d2" },
  { seconds: 60, label: "1m", labelJa: "1 \u5206" },
  { seconds: 120, label: "2m", labelJa: "2 \u5206" },
  { seconds: 300, label: "5m", labelJa: "5 \u5206" },
] as const;

const optionLabel = (value: string) => translateKnownMessage(value);
const translationEngineLabel = (
  value: ConfirmationSettings["translation_engine"],
) =>
  t(
    value === "FoundationModel"
      ? "Apple Intelligence Foundation Model"
      : "Apple Translation Framework",
  );
const statusApplicationPositionLabel = (
  value: NonNullable<ConfirmationSettings["status_application_position"]>,
) =>
  t(value === "NextToTimestamp" ? "Next to timestamp" : "Above actions");
const blueskyFetchIntervalLabel = (
  option: (typeof BLUESKY_FETCH_INTERVAL_OPTIONS)[number],
) => (appLocale === "ja" ? option.labelJa : option.label);

export function AccountSettings() {
  const snapshot = useAppStore((state) => state.snapshot);
  const switchAccount = useAppStore((state) => state.switchAccount);
  const logoutAccount = useAppStore((state) => state.logoutAccount);
  const refreshAccounts = useAppStore((state) => state.refreshAccounts);
  const save = useAppStore((state) => state.saveSetting);
  const mutationStates = useAppStore((state) => state.mutationStates);
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
                  disabled={mutationStates["account:switch"]?.phase === "pending"}
                >
                  {t("Activate")}
                </button>
              ) : null}
              <button
                className="btn btn-error btn-sm h-8 min-h-8 px-4 text-sm font-normal"
                onClick={() => void logoutAccount(account.acct)}
                disabled={
                  mutationStates[`account:logout:${account.acct}`]?.phase ===
                    "confirming" ||
                  mutationStates[`account:logout:${account.acct}`]?.phase ===
                    "pending"
                }
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

export function AppearanceSettingsPanel() {
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

export function BehaviorSettingsPanel() {
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
          label={t("Show post application")}
          checked={settings.show_status_application ?? false}
          onChange={(show_status_application) =>
            update({ show_status_application })
          }
        />
        {settings.show_status_application ? (
          <SelectRow
            label={t("Post application position")}
            value={settings.status_application_position ?? "AboveActions"}
            values={["AboveActions", "NextToTimestamp"]}
            optionLabel={statusApplicationPositionLabel}
            onChange={(status_application_position) =>
              update({ status_application_position })
            }
          />
        ) : null}
        <p className="col-span-2 text-xs text-subtext0">
          {t(
            "Due to Fediverse limitations, remote instances or servers may not provide post application data.",
          )}
        </p>
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

export function PerformanceSettingsPanel() {
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

export function NotificationSettingsPanel() {
  const [mutedAccounts, setMutedAccounts] = React.useState<
    NotificationMutedAccountSummary[]
  >([]);
  const [loading, setLoading] = React.useState(true);
  const runMutation = useAppStore((state) => state.runMutation);
  const mutationStates = useAppStore((state) => state.mutationStates);

  const loadMutedAccounts = React.useCallback(async () => {
    setLoading(true);
    try {
      const accounts = await invokeTypedReadCommand(
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
    const result = await runMutation(`notification:unmute:${key}`, {
      execute: () =>
        invokeTypedCommand("set_account_notification_mute", {
          request: {
            accountId: account.accountId,
            serverDomain: account.serverDomain,
            muted: false,
          },
        }),
      isUncertain: isResponseLossError,
    });
    if (result !== undefined) {
      setMutedAccounts((current) =>
        current.filter((item) => notificationMuteKey(item) !== key),
      );
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
            const mutation = mutationStates[`notification:unmute:${key}`];
            const busy =
              mutation?.phase === "confirming" || mutation?.phase === "pending";
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
                  {mutation?.phase === "uncertain" ? (
                    <div className="mt-1 text-xs text-yellow" role="alert">
                      {t("The result is uncertain. Refresh before retrying.")}
                    </div>
                  ) : null}
                </div>
                <button
                  className="btn btn-secondary btn-sm h-8 min-h-8 shrink-0 px-4 text-sm font-normal"
                  disabled={busy || mutation?.phase === "uncertain"}
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
