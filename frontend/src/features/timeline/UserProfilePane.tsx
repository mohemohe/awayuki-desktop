import React from "react";
import { Loader2, MoreHorizontal } from "lucide-react";
import {
  invokeTypedCommand,
  invokeTypedCommandWithOperationId,
  invokeTypedReadCommandWithOperationId,
} from "../../api/tauri";
import { isResponseLossError } from "../../api/ipcErrors";
import { canonicalStatusKey } from "../../domain/timelineEntities";
import { t } from "../../i18n";
import { useAppStore } from "../../store/appStore";
import { cancelQuoteConsumer } from "../../store/actions/timelineQueryActions";
import { measureNextPaint } from "../../utils/renderMetrics";
import type {
  AccountProfileSummary,
  ColumnSummary,
  TimelineStatus,
} from "../../types/app";
import { openExternalUrl } from "../../utils/browser";
import { formatCompactNumber } from "../../utils/format";
import { uniqueMediaSources } from "../../utils/media";
import {
  frontendRequestScheduler,
  RequestCancelledError,
} from "../../utils/requestScheduler";
import { useRetriedMediaSource } from "../../utils/useRetriedMediaSource";
import { confirmFollowAction } from "../../utils/confirmation";
import { Avatar } from "../../components/common/Avatar";
import {
  CustomEmojiText,
  StatusHtmlWithCustomEmojis,
} from "../../components/common/CustomEmoji";
import { PostMenuPopover } from "../../components/common/PostMenuPopover";
import { TimelineStatusList } from "./TimelineStatusList";

const EMPTY_STATUSES: TimelineStatus[] = [];

function elapsedUiMs(startedAt: number) {
  return (performance.now() - startedAt).toFixed(1);
}

function uncertainUiMutation(error: unknown) {
  return isResponseLossError(error);
}

export function UserProfilePane({
  column,
  scrollTopRequest,
}: {
  column: ColumnSummary;
  scrollTopRequest: number;
}) {
  const activeAccount = useAppStore((state) => {
    const accounts = state.snapshot?.accounts ?? [];
    return (
      accounts.find((account) => account.acct === state.snapshot?.activeAcct) ??
      accounts.find((account) => account.isActive)
    );
  });
  const confirmationSettings = useAppStore(
    (state) => state.snapshot?.settings.confirmation,
  );
  const requestConfirmation = useAppStore((state) => state.requestConfirmation);
  const openUserBookmarksPane = useAppStore(
    (state) => state.openUserBookmarksPane,
  );
  const replaceTimelineSlice = useAppStore(
    (state) => state.replaceTimelineSlice,
  );
  const removeTimelineSlices = useAppStore(
    (state) => state.removeTimelineSlices,
  );
  const target = column.profile!;
  const relationshipMutationKey = `profile:${target.serverDomain}:${target.accountId}:relationship`;
  const notificationMutationKey = `profile:${target.serverDomain}:${target.accountId}:notification`;
  const runMutation = useAppStore((state) => state.runMutation);
  const relationshipMutation = useAppStore(
    (state) => state.mutationStates[relationshipMutationKey],
  );
  const notificationMutation = useAppStore(
    (state) => state.mutationStates[notificationMutationKey],
  );
  const postsSliceId = `profile:${column.id}:posts`;
  const pinnedSliceId = `profile:${column.id}:pinned`;
  const mediaSliceId = `profile:${column.id}:media`;
  const [profile, setProfile] = React.useState<AccountProfileSummary | null>(
    null,
  );
  const posts = useAppStore(
    (state) => state.timelines[postsSliceId] ?? EMPTY_STATUSES,
  );
  const pinnedPosts = useAppStore(
    (state) => state.timelines[pinnedSliceId] ?? EMPTY_STATUSES,
  );
  const mediaPosts = useAppStore(
    (state) => state.timelines[mediaSliceId] ?? EMPTY_STATUSES,
  );
  const [tab, setTab] = React.useState<"posts" | "media">("posts");
  const [loading, setLoading] = React.useState(true);
  const [loadingMore, setLoadingMore] = React.useState(false);
  const [postsHasMore, setPostsHasMore] = React.useState(true);
  const [postsCursor, setPostsCursor] = React.useState<string | null>(null);
  const [mediaHasMore, setMediaHasMore] = React.useState(true);
  const [mediaCursor, setMediaCursor] = React.useState<string | null>(null);
  const [menuPosition, setMenuPosition] = React.useState<{
    top: number;
    right: number;
  } | null>(null);
  const profileLifecycleGenerationRef = React.useRef(0);
  const loadMoreOperationIdRef = React.useRef<string | null>(null);

  React.useEffect(() => {
    profileLifecycleGenerationRef.current += 1;
    let disposed = false;
    setLoading(true);
    setLoadingMore(false);
    setPostsHasMore(true);
    setPostsCursor(null);
    setMediaHasMore(true);
    setMediaCursor(null);
    measureNextPaint("profile:open");
    const paneStartedAt = performance.now();
    const paneContext = `column=${column.id} account=${target.accountId} server=${target.serverDomain}`;
    console.info(`[awayuki][ui-profile-pane] start ${paneContext}`);
    const profileRequest = {
      accountId: target.accountId,
      serverDomain: target.serverDomain,
      sourceAcct: target.sourceAcct ?? undefined,
    };
    const timelineRequest = (
      pinned: boolean,
      onlyMedia: boolean,
      limit: number,
      offset = 0,
      cursor?: string | null,
    ) => ({
      accountId: target.accountId,
      serverDomain: target.serverDomain,
      sourceAcct: target.sourceAcct ?? undefined,
      quoteConsumerId: column.id,
      pinned,
      onlyMedia,
      limit,
      offset,
      cursor: cursor ?? undefined,
    });

    const loadTimelineSlice = async (
      slice: "pinned" | "posts" | "media",
      pinned: boolean,
      onlyMedia: boolean,
      limit: number,
      operationId: string,
    ) => {
      const startedAt = performance.now();
      console.info(
        `[awayuki][ui-profile-pane] account_timeline_start ${paneContext} slice=${slice} pinned=${pinned} only_media=${onlyMedia} limit=${limit}`,
      );
      try {
        const page = await invokeTypedReadCommandWithOperationId(
          "account_timeline",
          {
            request: timelineRequest(pinned, onlyMedia, limit),
          },
          operationId,
        );
        console.info(
          `[awayuki][ui-profile-pane] account_timeline_success ${paneContext} slice=${slice} count=${page.statuses.length} has_more=${page.hasMore} duration_ms=${elapsedUiMs(startedAt)} total_ms=${elapsedUiMs(paneStartedAt)}`,
        );
        return page;
      } catch (error) {
        console.error(
          `[awayuki][ui-profile-pane] account_timeline_error ${paneContext} slice=${slice} duration_ms=${elapsedUiMs(startedAt)} total_ms=${elapsedUiMs(paneStartedAt)} error=${String(error)}`,
        );
        throw error;
      }
    };

    const schedule = <T,>(
      resource: "identity" | "pinned" | "posts" | "media",
      task: (signal: AbortSignal, operationId: string) => Promise<T>,
      priority: number,
    ) => {
      const key = `profile:${column.id}:${resource}`;
      return frontendRequestScheduler.schedule<T>(
        { key, lane: "profile", priority },
        async (context) => {
          const operationId = crypto.randomUUID();
          const cancel = () => {
            void invokeTypedCommand("cancel_timeline_query", {
              request: { targetOperationId: operationId },
            }).catch(() => undefined);
          };
          context.signal.addEventListener("abort", cancel, { once: true });
          useAppStore.setState((state) => ({
            resourceStates: {
              ...state.resourceStates,
              [key]: { generation: context.generation, phase: "loading" },
            },
          }));
          try {
            const value = await task(context.signal, operationId);
            if (!context.isCurrent()) throw new RequestCancelledError(key);
            useAppStore.setState((state) =>
              state.resourceStates[key]?.generation === context.generation
                ? {
                    resourceStates: {
                      ...state.resourceStates,
                      [key]: {
                        generation: context.generation,
                        phase: "succeeded",
                      },
                    },
                  }
                : {},
            );
            return value;
          } catch (error) {
            const cancelled = error instanceof RequestCancelledError;
            useAppStore.setState((state) =>
              state.resourceStates[key]?.generation === context.generation
                ? {
                    resourceStates: {
                      ...state.resourceStates,
                      [key]: {
                        generation: context.generation,
                        phase: cancelled ? "cancelled" : "failed",
                        ...(cancelled ? {} : { error: String(error) }),
                      },
                    },
                  }
                : {},
            );
            throw error;
          } finally {
            context.signal.removeEventListener("abort", cancel);
          }
        },
      );
    };

    const profileStartedAt = performance.now();
    console.info(
      `[awayuki][ui-profile-pane] account_profile_start ${paneContext}`,
    );
    const profilePromise = schedule(
      "identity",
      (_signal, operationId) =>
        invokeTypedReadCommandWithOperationId("account_profile", {
          request: profileRequest,
        }, operationId),
      100,
    );
    const pinnedPromise = schedule(
      "pinned",
      (_signal, operationId) =>
        loadTimelineSlice("pinned", true, false, 40, operationId),
      90,
    );
    const postsPromise = schedule(
      "posts",
      (_signal, operationId) =>
        loadTimelineSlice("posts", false, false, 80, operationId),
      80,
    );
    const mediaPromise = schedule(
      "media",
      (_signal, operationId) =>
        loadTimelineSlice("media", false, true, 80, operationId),
      70,
    );

    const reportRequestError = (error: unknown) => {
      if (!disposed && !(error instanceof RequestCancelledError)) {
        useAppStore.setState({ error: String(error) });
      }
    };

    void profilePromise.then(
      (value) => {
        if (disposed) return;
        setProfile(value);
        setLoading(false);
        console.info(
          `[awayuki][ui-profile-pane] account_profile_success ${paneContext} duration_ms=${elapsedUiMs(profileStartedAt)} total_ms=${elapsedUiMs(paneStartedAt)}`,
        );
      },
      (error) => {
        if (disposed) return;
        reportRequestError(error);
        setLoading(false);
      },
    );
    void pinnedPromise.then(
      (value) => {
        if (!disposed) replaceTimelineSlice(pinnedSliceId, value.statuses, 40);
      },
      reportRequestError,
    );
    void postsPromise.then(
      (value) => {
        if (disposed) return;
        replaceTimelineSlice(postsSliceId, value.statuses, value.statuses.length);
        setPostsHasMore(value.hasMore);
        setPostsCursor(value.nextCursor ?? null);
      },
      reportRequestError,
    );
    void mediaPromise.then(
      (value) => {
        if (disposed) return;
        replaceTimelineSlice(mediaSliceId, value.statuses, value.statuses.length);
        setMediaHasMore(value.hasMore);
        setMediaCursor(value.nextCursor ?? null);
      },
      reportRequestError,
    );

    void Promise.allSettled([
      profilePromise,
      pinnedPromise,
      postsPromise,
      mediaPromise,
    ]).then(([, pinned, posts, media]) => {
      useAppStore.setState({
        requestMetrics: frontendRequestScheduler.metrics(),
      });
      if (disposed) return;
      console.info(
        `[awayuki][ui-profile-pane] complete ${paneContext} pinned=${pinned.status === "fulfilled" ? pinned.value.statuses.length : "error"} posts=${posts.status === "fulfilled" ? posts.value.statuses.length : "error"} media=${media.status === "fulfilled" ? media.value.statuses.length : "error"} duration_ms=${elapsedUiMs(paneStartedAt)}`,
      );
    });
    return () => {
      disposed = true;
      profileLifecycleGenerationRef.current += 1;
      const loadMoreOperationId = loadMoreOperationIdRef.current;
      if (loadMoreOperationId) {
        loadMoreOperationIdRef.current = null;
        void invokeTypedCommand("cancel_timeline_query", {
          request: { targetOperationId: loadMoreOperationId },
        }).catch(() => undefined);
      }
      frontendRequestScheduler.cancelPrefix(`profile:${column.id}:`);
      void cancelQuoteConsumer(column.id).catch(() => undefined);
      removeTimelineSlices([postsSliceId, pinnedSliceId, mediaSliceId]);
      console.info(
        `[awayuki][ui-profile-pane] dispose ${paneContext} duration_ms=${elapsedUiMs(paneStartedAt)}`,
      );
    };
  }, [
    mediaSliceId,
    column.id,
    pinnedSliceId,
    postsSliceId,
    removeTimelineSlices,
    replaceTimelineSlice,
    target.accountId,
    target.serverDomain,
    target.sourceAcct,
  ]);

  const headerSources = React.useMemo(
    () => uniqueMediaSources([profile?.header]),
    [profile?.header],
  );
  const headerImage = useRetriedMediaSource(headerSources);

  const followAction = async () => {
    if (!profile || profile.isSelf) return;
    const action = profile.relationship?.following ? "unfollow" : "follow";
    const relationship = await runMutation(relationshipMutationKey, {
      confirm: () =>
        confirmFollowAction(
          confirmationSettings,
          requestConfirmation,
          profile,
          action,
        ),
      execute: (operationId) =>
        invokeTypedCommandWithOperationId("account_follow_action", {
          request: {
            accountId: profile.id,
            serverDomain: profile.serverDomain,
            targetAcct: profile.acct,
            actingAccountAcct: activeAccount?.acct ?? "",
            action,
          },
        }, operationId),
      isUncertain: uncertainUiMutation,
    });
    if (relationship) {
      setProfile({ ...profile, relationship });
    }
  };
  const setDesktopNotificationMuted = async (muted: boolean) => {
    if (!profile) return;
    const notificationMuted = await runMutation(notificationMutationKey, {
      execute: (operationId) =>
        invokeTypedCommandWithOperationId("set_account_notification_mute", {
          request: {
            accountId: profile.id,
            serverDomain: profile.serverDomain,
            muted,
          },
        }, operationId),
      isUncertain: uncertainUiMutation,
    });
    if (notificationMuted !== undefined) {
      setProfile({ ...profile, notificationMuted });
    }
  };
  const accountModerationAction = async (
    action: "mute" | "unmute" | "block" | "unblock",
  ) => {
    if (!profile || profile.isSelf) return;
    const relationship = await runMutation(relationshipMutationKey, {
      execute: (operationId) =>
        invokeTypedCommandWithOperationId("account_follow_action", {
          request: {
            accountId: profile.id,
            serverDomain: profile.serverDomain,
            targetAcct: profile.acct,
            actingAccountAcct: activeAccount?.acct ?? "",
            action,
          },
        }, operationId),
      isUncertain: uncertainUiMutation,
    });
    if (relationship) {
      setProfile({ ...profile, relationship });
    }
  };
  const openProfileUrl = () => {
    if (!profile?.url) return;
    void openExternalUrl(profile.url).catch((error) =>
      useAppStore.setState({ error: String(error) }),
    );
  };
  const toggleMenu = (event: React.MouseEvent<HTMLButtonElement>) => {
    const rect = event.currentTarget.getBoundingClientRect();
    setMenuPosition((current) =>
      current
        ? null
        : {
            top: Math.max(
              8,
              Math.min(rect.bottom + 4, window.innerHeight - 196),
            ),
            right: Math.max(8, window.innerWidth - rect.right),
          },
    );
  };
  const visiblePosts = tab === "media" ? mediaPosts : posts;
  const hasMore = tab === "media" ? mediaHasMore : postsHasMore;
  const loadMoreProfileStatuses = React.useCallback(async () => {
    if (loadingMore || loadMoreOperationIdRef.current || !hasMore) return;
    const media = tab === "media";
    const sliceId = media ? mediaSliceId : postsSliceId;
    const cursor = media ? mediaCursor : postsCursor;
    const current = useAppStore.getState().timelines[sliceId] ?? [];
    const operationId = crypto.randomUUID();
    const lifecycleGeneration = profileLifecycleGenerationRef.current;
    const isCurrentLoad = () =>
      profileLifecycleGenerationRef.current === lifecycleGeneration &&
      loadMoreOperationIdRef.current === operationId;
    loadMoreOperationIdRef.current = operationId;
    setLoadingMore(true);
    try {
      const page = await invokeTypedReadCommandWithOperationId(
        "account_timeline",
        {
          request: {
            accountId: target.accountId,
            serverDomain: target.serverDomain,
            sourceAcct: target.sourceAcct ?? undefined,
            quoteConsumerId: column.id,
            pinned: false,
            onlyMedia: media,
            limit: 80,
            offset: current.length,
            cursor: cursor ?? undefined,
          },
        },
        operationId,
      );
      if (!isCurrentLoad()) return;
      const seen = new Set(current.map(canonicalStatusKey));
      const merged = [
        ...current,
        ...page.statuses.filter((status) => {
          const key = canonicalStatusKey(status);
          if (seen.has(key)) return false;
          seen.add(key);
          return true;
        }),
      ];
      const nextCursor = page.nextCursor ?? null;
      const madeProgress =
        nextCursor !== null ? nextCursor !== cursor : merged.length > current.length;
      const pageHasMore = page.hasMore && madeProgress;
      replaceTimelineSlice(sliceId, merged, merged.length);
      if (media) {
        setMediaHasMore(pageHasMore);
        setMediaCursor(nextCursor);
      } else {
        setPostsHasMore(pageHasMore);
        setPostsCursor(nextCursor);
      }
    } catch (error) {
      if (!isCurrentLoad()) return;
      console.error(
        `[awayuki][ui-profile-pane] account_timeline_load_more_error column=${column.id} account=${target.accountId} server=${target.serverDomain} slice=${media ? "media" : "posts"} error=${String(error)}`,
      );
      useAppStore.setState({ error: String(error) });
    } finally {
      if (isCurrentLoad()) {
        loadMoreOperationIdRef.current = null;
        setLoadingMore(false);
      }
    }
  }, [
    column.id,
    hasMore,
    loadingMore,
    mediaCursor,
    mediaSliceId,
    postsCursor,
    postsSliceId,
    replaceTimelineSlice,
    tab,
    target.accountId,
    target.serverDomain,
    target.sourceAcct,
  ]);
  const profileTimelineStatuses = React.useMemo(() => {
    if (tab === "media") return visiblePosts;
    const seen = new Set<string>();
    return [...pinnedPosts, ...visiblePosts].filter((status) => {
      const key = canonicalStatusKey(status);
      if (seen.has(key)) return false;
      seen.add(key);
      return true;
    });
  }, [pinnedPosts, tab, visiblePosts]);

  if (loading && !profile) {
    return (
      <div className="grid h-full place-items-center text-sm text-subtext0">
        <Loader2 className="h-4 w-4 animate-spin" />
      </div>
    );
  }

  const displayName = profile?.displayName || target.displayName || target.acct;
  const acct = profile?.acct || target.acct;
  const accountEmojis = profile?.accountEmojis ?? [];
  const acctLabel = `@${acct.replace(/^@/, "")}`;
  const relationshipBusy =
    relationshipMutation?.phase === "confirming" ||
    relationshipMutation?.phase === "pending";
  const notificationBusy = notificationMutation?.phase === "pending";

  const profileScrollHeader = (
    <>
      <div className="relative z-0 h-32 shrink-0 overflow-hidden border-b border-surface0 bg-base-200">
        {headerImage.src && !headerImage.failed ? (
          <img
            key={headerImage.key}
            src={headerImage.src}
            alt=""
            className={`h-full w-full object-cover ${headerImage.loaded ? "" : "opacity-0"}`}
            onLoad={headerImage.onLoad}
            onError={headerImage.onError}
          />
        ) : null}
        {headerImage.retrying ? (
          <div className="absolute inset-0 grid place-items-center bg-base-200/70 text-overlay0">
            <Loader2 className="h-4 w-4 animate-spin" />
          </div>
        ) : null}
      </div>
      <div className="relative z-10 shrink-0 border-b border-surface0 bg-base px-3 py-3">
        <div className="relative z-20 mb-1 flex min-w-0 items-center justify-end gap-2 pl-20">
          <div className="rounded absolute left-0">
            <Avatar
              sources={[profile?.avatar, target.avatar]}
              label={displayName}
              size="xxl"
            />
          </div>
          {profile?.relationship?.followedBy ||
          (profile && !profile.isSelf) ? (
            <div className="flex min-w-0 shrink-0 flex-col items-end gap-1">
              {profile?.relationship?.followedBy ? (
                <span className="badge badge-sm border-blue bg-blue text-primary-content">
                  {t("Follows you")}
                </span>
              ) : null}
              {profile && !profile.isSelf ? (
                <span
                  className={`badge badge-sm ${profile.notificationMuted ? "border-yellow bg-yellow text-warning-content" : "border-transparent bg-blue text-primary-content"}`}
                  title={
                    profile.notificationMuted
                      ? t("Desktop notifications disabled")
                      : t("Desktop notifications enabled")
                  }
                >
                  {profile.notificationMuted ? t("Notify off") : t("Notify on")}
                </span>
              ) : null}
            </div>
          ) : null}
          <button
            className={`btn btn-sm shrink-0 ${profile?.relationship?.following ? "border-red bg-red text-error-content hover:border-red hover:bg-red hover:text-error-content" : "btn-secondary"}`}
            disabled={
              profile?.isSelf ||
              relationshipBusy ||
              !activeAccount?.capabilities.relationship.follow
            }
            onClick={() => void followAction()}
          >
            {profile?.isSelf
              ? t("This is you")
              : profile?.relationship?.following
                ? t("Unfollow")
                : profile?.relationship?.requested
                  ? t("Requested")
                  : t("Follow")}
          </button>
          <button
            className="btn btn-ghost btn-sm shrink-0 px-2"
            onClick={toggleMenu}
            title={t("More")}
            disabled={!profile}
            data-post-menu-trigger
          >
            <MoreHorizontal className="h-4 w-4" />
          </button>
          {menuPosition && profile ? (
            <PostMenuPopover
              position={menuPosition}
              onClose={() => setMenuPosition(null)}
              widthClassName="w-60"
              items={[
                {
                  label: t("Open in browser"),
                  action: openProfileUrl,
                  disabled: !profile.url,
                },
                {
                  label: t("Search this user's bookmarks"),
                  action: () =>
                    openUserBookmarksPane({
                      accountId: profile.id,
                      serverDomain: profile.serverDomain,
                      acct: profile.acct,
                      displayName: profile.displayName,
                      avatar: profile.avatar,
                    }),
                },
                {
                  label: profile.notificationMuted
                    ? t("Enable desktop notifications")
                    : t("Disable desktop notifications"),
                  action: () =>
                    void setDesktopNotificationMuted(
                      !profile.notificationMuted,
                    ),
                  disabled: profile.isSelf || notificationBusy,
                },
                {
                  label: profile.relationship?.muting
                    ? t("Unmute {acct}", { acct: acctLabel })
                    : t("Mute {acct}", { acct: acctLabel }),
                  action: () =>
                    void accountModerationAction(
                      profile.relationship?.muting ? "unmute" : "mute",
                    ),
                  disabled:
                    profile.isSelf ||
                    relationshipBusy ||
                    !activeAccount?.capabilities.relationship.mute,
                },
                {
                  label: profile.relationship?.blocking
                    ? t("Unblock {acct}", { acct: acctLabel })
                    : t("Block {acct}", { acct: acctLabel }),
                  action: () =>
                    void accountModerationAction(
                      profile.relationship?.blocking ? "unblock" : "block",
                    ),
                  disabled:
                    profile.isSelf ||
                    relationshipBusy ||
                    !activeAccount?.capabilities.relationship.block,
                },
              ]}
            />
          ) : null}
        </div>
        <div className="user-name mt-3 font-semibold text-text text-xl">
          <CustomEmojiText text={displayName} emojis={accountEmojis} />
        </div>
        <div className="user-acct text-sm text-subtext0">
          @{acct.replace(/^@/, "")}
        </div>
        {profile?.note ? (
          <StatusHtmlWithCustomEmojis
            className="status-content mt-3 text-sm leading-6"
            html={profile.note}
            emojis={accountEmojis}
          />
        ) : null}
        {profile?.fields.length ? (
          <div className="mt-3 overflow-hidden rounded border border-surface0">
            {profile.fields.map((field) => (
              <div
                key={field.name}
                className="border-b border-surface0 px-3 py-2 last:border-b-0"
              >
                <div className="text-xs text-subtext0">{field.name}</div>
                <StatusHtmlWithCustomEmojis
                  className="status-content text-sm"
                  html={field.value}
                  emojis={accountEmojis}
                />
              </div>
            ))}
          </div>
        ) : null}
        {profile ? (
          <div className="mt-3 flex gap-5 text-sm text-subtext0">
            <span>
              <b className="text-text">
                {formatCompactNumber(profile.statusesCount)}
              </b>{" "}
              {t("Posts")}
            </span>
            <span>
              <b className="text-text">
                {formatCompactNumber(profile.followingCount)}
              </b>{" "}
              {t("Following")}
            </span>
            <span>
              <b className="text-text">
                {formatCompactNumber(profile.followersCount)}
              </b>{" "}
              {t("Followers")}
            </span>
          </div>
        ) : null}
      </div>
      <div className="grid shrink-0 grid-cols-2 border-b border-surface0">
        <button
          className={`h-10 border-b-2 text-sm ${tab === "posts" ? "border-blue text-text" : "border-transparent text-subtext0"}`}
          onClick={() => setTab("posts")}
        >
          {t("Posts")}
        </button>
        <button
          className={`h-10 border-b-2 text-sm ${tab === "media" ? "border-blue text-text" : "border-transparent text-subtext0"}`}
          onClick={() => setTab("media")}
        >
          {t("Media")}
        </button>
      </div>
    </>
  );

  return (
    <div
      className="flex h-full min-h-0 flex-col bg-base text-sm"
      data-profile-scroll-surface
    >
      <TimelineStatusList
        column={column}
        statuses={profileTimelineStatuses}
        virtualized
        scrollHeader={profileScrollHeader}
        emptyState={
          <div className="p-4 text-xs text-subtext0">
            {t("No statuses loaded.")}
          </div>
        }
        scrollTopRequest={scrollTopRequest}
        isLoading={false}
        isLoadingMore={loadingMore}
        hasMore={hasMore}
        onLoadMore={() => void loadMoreProfileStatuses()}
        onNearTopChange={() => undefined}
        onScrollTopComplete={() => undefined}
      />
    </div>
  );
}
