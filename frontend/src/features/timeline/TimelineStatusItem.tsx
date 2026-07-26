import React from "react";
import {
  Bookmark,
  MessageCircle,
  MoreHorizontal,
  Quote,
  Repeat2,
  Star,
} from "lucide-react";
import { accountSourceCssColor } from "../../constants/accountSourceColors";
import { canonicalStatusKey } from "../../domain/timelineEntities";
import { t } from "../../i18n";
import { useAppStore } from "../../store/appStore";
import type { ColumnSummary, TimelineStatus } from "../../types/app";
import { copyToClipboard, openExternalUrl } from "../../utils/browser";
import { formatTime, statusPlainText, statusUrl } from "../../utils/format";
import { thumbnailMediaSources } from "../../utils/media";
import { Avatar } from "../../components/common/Avatar";
import { CustomEmojiText } from "../../components/common/CustomEmoji";
import { PostMenuPopover } from "../../components/common/PostMenuPopover";
import { VisibilityIcon } from "../../components/common/VisibilityIcon";
import { NotificationMeta } from "./NotificationMeta";
import { MediaThumbnail, statusDisplayCreatedAt } from "./TimelineMedia";
import { StatusPoll } from "./TimelinePoll";
import { QuotePreview, StatusContentBlock } from "./TimelineStatusContent";
import {
  QuoteLinkPreview,
  statusFontSizeClass,
  statusHoverBackgroundClass,
  statusItemStyle,
  statusVisibilityBackgroundClass,
} from "./TimelineStatusHelpers";

export function StatusItem({
  column,
  status,
  threadDepth = 0,
}: {
  column: ColumnSummary;
  status: TimelineStatus;
  threadDepth?: number;
}) {
  const action = useAppStore((state) => state.action);
  const votePoll = useAppStore((state) => state.votePoll);
  const deleteStatus = useAppStore((state) => state.deleteStatus);
  const replyStatus = useAppStore((state) => state.replyStatus);
  const quoteStatus = useAppStore((state) => state.quoteStatus);
  const beginEditStatus = useAppStore((state) => state.beginEditStatus);
  const openThreadPane = useAppStore((state) => state.openThreadPane);
  const openAirContextPane = useAppStore(
    (state) => state.openAirContextPane,
  );
  const openUserPane = useAppStore((state) => state.openUserPane);
  const openMediaPreview = useAppStore((state) => state.openMediaPreview);
  const requestConfirmation = useAppStore((state) => state.requestConfirmation);
  const mutation = useAppStore(
    (state) => state.statusMutations[canonicalStatusKey(status)],
  );
  const statusMutationPending = mutation?.phase === "pending";
  const activeAccount = useAppStore((state) => {
    const accounts = state.snapshot?.accounts ?? [];
    return (
      accounts.find((account) => account.acct === state.snapshot?.activeAcct) ??
      accounts.find((account) => account.isActive)
    );
  });
  const nsfwBehavior = useAppStore(
    (state) => state.snapshot?.settings.appearance.nsfw_behavior ?? "Hide",
  );
  const fontSize = useAppStore(
    (state) => state.snapshot?.settings.appearance.font_size ?? "Medium",
  );
  const cwBehavior = useAppStore(
    (state) => state.snapshot?.settings.appearance.cw_behavior ?? "Hide",
  );
  const displayMode = useAppStore(
    (state) => state.snapshot?.settings.appearance.display_mode ?? "StarryEyes",
  );
  const visibilityBackgroundEnabled = useAppStore(
    (state) =>
      state.snapshot?.settings.appearance.visibility_background_enabled ??
      false,
  );
  const showStatusApplication = useAppStore(
    (state) =>
      state.snapshot?.settings.confirmation.show_status_application ?? false,
  );
  const statusApplicationPosition = useAppStore(
    (state) =>
      state.snapshot?.settings.confirmation.status_application_position ??
      "AboveActions",
  );
  const sourceBorderColor = useAppStore((state) =>
    accountSourceCssColor(
      status.sourceAcct
        ? state.snapshot?.settings.accountSourceColors[status.sourceAcct]
        : undefined,
    ),
  );
  const mediaSourcePreference = useAppStore(
    (state) => state.snapshot?.settings.confirmation.media_source ?? "Local",
  );
  const [mystiqueExpanded, setMystiqueExpanded] = React.useState(false);
  const [mediaVisibility, setMediaVisibility] = React.useState<
    Record<string, boolean>
  >({});
  const [menuPosition, setMenuPosition] = React.useState<{
    top: number;
    right: number;
  } | null>(null);
  const url = statusUrl(status);
  const fontSizeClass = statusFontSizeClass(fontSize);
  const visibilityBackgroundClass = statusVisibilityBackgroundClass(
    visibilityBackgroundEnabled,
    status.visibility,
  );
  const hoverBackgroundClass = statusHoverBackgroundClass(
    visibilityBackgroundClass,
  );
  const statusApplicationLabel = showStatusApplication
    ? status.applicationName?.trim()
    : undefined;
  const showStatusApplicationNextToTimestamp =
    statusApplicationPosition === "NextToTimestamp";
  const showStatusApplicationAboveActions =
    statusApplicationPosition === "AboveActions";
  const isMystique = displayMode === "Mystique";
  const isCompact = isMystique && !mystiqueExpanded;
  const threadIndent = threadDepth > 0 ? 12 + threadDepth * 18 : undefined;
  const threadLineLeft = threadDepth > 0 ? 5 + threadDepth * 18 : undefined;
  const statusStyle = statusItemStyle(threadIndent, sourceBorderColor);
  const openThread = (event: React.MouseEvent<HTMLElement>) => {
    event.stopPropagation();
    openThreadPane(status);
  };
  React.useEffect(() => {
    setMystiqueExpanded(false);
  }, [displayMode, status.id, status.notificationId]);
  const canManageStatus =
    activeAccount?.accountId === status.accountId &&
    activeAccount.serverDomain === status.serverDomain;
  const copyText = () =>
    void copyToClipboard(statusPlainText(status)).catch((error) =>
      useAppStore.setState({ error: String(error) }),
    );
  const copyUrl = () => {
    if (!url) return;
    void copyToClipboard(url).catch((error) =>
      useAppStore.setState({ error: String(error) }),
    );
  };
  const openUrl = () => {
    if (!url) return;
    void openExternalUrl(url).catch((error) =>
      useAppStore.setState({ error: String(error) }),
    );
  };
  const editPost = () => {
    beginEditStatus(status);
  };
  const deletePost = async () => {
    const confirmed = await requestConfirmation({
      title: t("Delete post"),
      message: t("Delete this post? This cannot be undone."),
      confirmLabel: t("Delete"),
      danger: true,
    });
    if (!confirmed) return;
    await deleteStatus(status);
  };
  const toggleMenu = (event: React.MouseEvent<HTMLButtonElement>) => {
    const rect = event.currentTarget.getBoundingClientRect();
    setMenuPosition((current) =>
      current
        ? null
        : {
            top: Math.min(rect.bottom + 4, window.innerHeight - 132),
            right: Math.max(8, window.innerWidth - rect.right),
          },
    );
  };
  const toggleMystiqueExpanded = (event: React.MouseEvent<HTMLElement>) => {
    if (!isMystique) return;
    const target = event.target as HTMLElement;
    if (
      target.closest(
        "a, button, details, input, label, menu, select, summary, textarea",
      )
    ) {
      return;
    }
    setMystiqueExpanded((current) => !current);
  };

  if (isCompact) {
    return (
      <article
        className={`${fontSizeClass} ${visibilityBackgroundClass} ${hoverBackgroundClass} relative grid min-h-6 max-w-full cursor-pointer grid-cols-[28px_minmax(0,1fr)_auto] items-center gap-2 overflow-hidden border-b border-l-2 border-surface0 border-l-transparent px-1.5 py-0.5`}
        style={statusStyle}
        onClick={() => setMystiqueExpanded(true)}
        title={t("Expand post")}
      >
        {threadLineLeft ? (
          <span
            className="pointer-events-none absolute bottom-0 top-0 w-px bg-surface1"
            style={{ left: threadLineLeft }}
          />
        ) : null}
        <Avatar src={status.avatar} label={status.displayName} size="md" />
        <div className="flex min-w-0 items-center gap-2">
          <span className="shrink-0 truncate font-semibold">
            <CustomEmojiText
              text={status.displayName || status.acct}
              emojis={status.accountEmojis}
            />
          </span>
          <span className="min-w-0 truncate font-extralight text-subtext0">
            {statusPlainText(status)}
          </span>
        </div>
        <button
          type="button"
          className="inline-flex min-w-0 shrink-0 items-center gap-1 text-xs text-overlay0 hover:text-blue"
          onClick={openThread}
          title={t("Open thread")}
        >
          {formatTime(statusDisplayCreatedAt(status))}
          {statusApplicationLabel ? (
            <span className="hidden max-w-28 truncate sm:inline">
              from {statusApplicationLabel}
            </span>
          ) : null}
        </button>
      </article>
    );
  }

  return (
    <article
      className={`${fontSizeClass} ${visibilityBackgroundClass} ${hoverBackgroundClass} relative max-w-full overflow-x-hidden border-b border-l-2 border-surface0 border-l-transparent px-3 py-3 ${isMystique ? "cursor-pointer" : ""}`}
      style={statusStyle}
      onClick={toggleMystiqueExpanded}
      title={isMystique ? t("Collapse post") : undefined}
    >
      {threadLineLeft ? (
        <span
          className="pointer-events-none absolute bottom-0 top-0 w-px bg-surface1"
          style={{ left: threadLineLeft }}
        />
      ) : null}
      <NotificationMeta status={status} onOpenUser={openUserPane} />
      <div className="flex max-w-full gap-3">
        <Avatar src={status.avatar} label={status.displayName} size="post" />
        <div className="min-w-0 max-w-full flex-1 overflow-x-hidden">
          <div className="flex min-w-0 items-baseline gap-1">
            <button
              className="truncate text-left font-semibold hover:text-blue"
              onClick={() => openUserPane(status)}
              title={t("Open profile")}
            >
              <CustomEmojiText
                text={status.displayName || status.acct}
                emojis={status.accountEmojis}
              />
            </button>
            <button
              className="truncate text-left text-xs text-subtext0 hover:text-blue"
              onClick={() => openUserPane(status)}
              title={t("Open profile")}
            >
              {status.acct}
            </button>
            <button
              type="button"
              className="ml-auto inline-flex shrink-0 items-center gap-1 text-xs text-overlay0 hover:text-blue"
              onClick={openThread}
              title={t("Open thread")}
            >
              <VisibilityIcon visibility={status.visibility} />
              {formatTime(statusDisplayCreatedAt(status))}
              {statusApplicationLabel &&
              showStatusApplicationNextToTimestamp ? (
                <span className="max-w-36 truncate text-overlay0">
                  from {statusApplicationLabel}
                </span>
              ) : null}
            </button>
          </div>
          <StatusContentBlock
            status={status}
            cwBehavior={cwBehavior}
            className="mt-1 max-w-full font-extralight"
          />
          {status.quote ? (
            <QuotePreview
              status={status.quote}
              onOpenUser={openUserPane}
              onOpenStatus={openThreadPane}
              onOpenMedia={openMediaPreview}
            />
          ) : status.quoteOriginalUrl ? (
            <QuoteLinkPreview url={status.quoteOriginalUrl} />
          ) : null}
          {status.media.length ? (
            <div className="mt-2 grid grid-cols-2 gap-1">
              {status.media.slice(0, 4).map((media) => {
                const sources = thumbnailMediaSources(
                  media,
                  mediaSourcePreference,
                );
                return sources.length ? (
                  <MediaThumbnail
                    key={media.id}
                    media={media}
                    sources={sources}
                    sensitive={status.sensitive}
                    visible={
                      mediaVisibility[media.id] ?? nsfwBehavior === "AlwaysShow"
                    }
                    onToggle={() =>
                      setMediaVisibility((current) => ({
                        ...current,
                        [media.id]: !(
                          current[media.id] ?? nsfwBehavior === "AlwaysShow"
                        ),
                      }))
                    }
                    onOpen={() => openMediaPreview(status, media)}
                  />
                ) : null;
              })}
            </div>
          ) : null}
          {status.poll ? (
            <StatusPoll
              status={status}
              votingSupported={
                activeAccount?.capabilities.status.vote ?? false
              }
              onVote={(choices) => votePoll(status, choices)}
            />
          ) : null}
          {statusApplicationLabel && showStatusApplicationAboveActions ? (
            <div className="mt-2 max-w-full truncate text-xs text-overlay0">
              from {statusApplicationLabel}
            </div>
          ) : null}
          <div className="mt-2.5 flex items-center gap-4 text-xs text-overlay0">
            <button
              className="inline-flex h-5 items-center gap-1 text-overlay0 hover:text-subtext0"
              title={t("Reply")}
              onClick={() => replyStatus(status)}
            >
              <MessageCircle className="h-3.5 w-3.5" />
              {status.repliesCount || ""}
            </button>
            {activeAccount?.capabilities.compose.quote ? (
              <button
                className="inline-flex h-5 items-center gap-1 text-overlay0 hover:text-subtext0"
                title={t("Quote")}
                onClick={() => quoteStatus(status)}
              >
                <Quote className="h-3.5 w-3.5" />
              </button>
            ) : null}
            {activeAccount?.capabilities.status.reblog ? (
              <button
                className={`inline-flex h-5 items-center gap-1 ${status.reblogged ? "text-green hover:text-green" : "text-overlay0 hover:text-subtext0"}`}
                title={t("Boost")}
                disabled={statusMutationPending}
                onClick={() =>
                  void action(
                    column,
                    status,
                    status.reblogged ? "unreblog" : "reblog",
                  )
                }
              >
                <Repeat2 className="h-3.5 w-3.5" />
                {status.reblogsCount || ""}
              </button>
            ) : null}
            {activeAccount?.capabilities.status.favourite ? (
              <button
                className={`inline-flex h-5 items-center gap-1 ${status.favourited ? "text-yellow hover:text-yellow" : "text-overlay0 hover:text-subtext0"}`}
                title={t("Favorite")}
                disabled={statusMutationPending}
                onClick={() =>
                  void action(
                    column,
                    status,
                    status.favourited ? "unfavourite" : "favourite",
                  )
                }
              >
                <Star className="h-3.5 w-3.5" />
                {status.favouritesCount || ""}
              </button>
            ) : null}
            {activeAccount?.capabilities.status.bookmark ? (
              <button
                className={`inline-flex h-5 items-center gap-1 ${status.bookmarked ? "text-blue hover:text-blue" : "text-overlay0 hover:text-subtext0"}`}
                title={t("Bookmark")}
                disabled={statusMutationPending}
                onClick={() =>
                  void action(
                    column,
                    status,
                    status.bookmarked ? "unbookmark" : "bookmark",
                  )
                }
              >
                <Bookmark className="h-3.5 w-3.5" />
              </button>
            ) : null}
            <div className="ml-auto flex items-center gap-2">
              <button
                className="btn btn-ghost btn-xs h-5 min-h-5 px-1 text-subtext0 hover:bg-surface1 hover:text-text"
                title={t("More")}
                onClick={toggleMenu}
                data-post-menu-trigger
              >
                <MoreHorizontal className="h-3.5 w-3.5" />
              </button>
              {menuPosition ? (
                <PostMenuPopover
                  position={menuPosition}
                  onClose={() => setMenuPosition(null)}
                  items={[
                    ...(canManageStatus
                      ? [
                          {
                            label: t("Edit post"),
                            action: editPost,
                            disabled: statusMutationPending,
                          },
                          {
                            label: t("Delete post"),
                            action: () => void deletePost(),
                            danger: true,
                            disabled: statusMutationPending,
                          },
                        ]
                      : []),
                    ...(status.notificationId
                      ? [
                          {
                            label: t("Find AIR context"),
                            action: () => openAirContextPane(status),
                            disabled: !status.notificationAccountId,
                          },
                        ]
                      : []),
                    { label: t("Copy text"), action: copyText },
                    { label: t("Copy URL"), action: copyUrl, disabled: !url },
                    {
                      label: t("Open in browser"),
                      action: openUrl,
                      disabled: !url,
                    },
                  ]}
                />
              ) : null}
            </div>
          </div>
          {mutation && mutation.phase !== "confirmed" ? (
            <div
              className={`mt-1 text-[10px] ${
                mutation.phase === "uncertain" || mutation.phase === "failed"
                  ? "text-yellow"
                  : "text-overlay0"
              }`}
              role={mutation.phase === "pending" ? "status" : "alert"}
              aria-live="polite"
            >
              {mutation.phase === "pending"
                ? t("Saving status action")
                : mutation.phase === "uncertain"
                  ? t("Status action result is uncertain")
                  : t("Status action failed and was rolled back")}
            </div>
          ) : null}
        </div>
      </div>
    </article>
  );
}
