import { beforeEach, describe, expect, it, vi } from "vitest";
import type {
  AppSnapshot,
  ColumnSummary,
  TimelineStatus,
} from "../types/app";
import { createTimelineEntityState } from "../domain/timelineEntities";
import { frontendRequestScheduler } from "../utils/requestScheduler";

const api = vi.hoisted(() => ({
  invokeCommand: vi.fn(),
  invokeReadCommand: vi.fn(),
}));

vi.mock("../api/tauri", () => api);

import { useAppStore } from "./appStore";

describe("appStore async resource generations", () => {
  beforeEach(() => {
    api.invokeCommand.mockReset();
    api.invokeReadCommand.mockReset();
    frontendRequestScheduler.resetForTest();
    resetStore([]);
  });

  it("discards a late result after the query changes", async () => {
    const oldColumn = column("search", "old");
    const newColumn = column("search", "new");
    resetStore([oldColumn]);
    const stale = deferred<TimelineStatus[]>();
    api.invokeReadCommand
      .mockImplementationOnce(() => stale.promise)
      .mockResolvedValueOnce([status("new-result")]);

    const oldLoad = useAppStore.getState().loadTimeline(oldColumn);
    await vi.waitFor(() => expect(api.invokeReadCommand).toHaveBeenCalledTimes(1));
    const newLoad = useAppStore.getState().loadTimeline(newColumn);
    await Promise.all([oldLoad, newLoad]);
    stale.resolve([status("stale-result")]);

    expect(useAppStore.getState().timelines.search?.map((item) => item.id)).toEqual([
      "new-result",
    ]);
    expect(useAppStore.getState().resourceStates["timeline:search"]?.phase).toBe(
      "succeeded",
    );
  });

  it("cancels a pane request when the dynamic pane closes", async () => {
    const dynamic = { ...column("dynamic", "needle"), dynamic: true };
    resetStore([], [dynamic]);
    const pending = deferred<TimelineStatus[]>();
    api.invokeReadCommand.mockImplementationOnce(() => pending.promise);

    const load = useAppStore.getState().loadTimeline(dynamic);
    await vi.waitFor(() => expect(api.invokeReadCommand).toHaveBeenCalledTimes(1));
    useAppStore.getState().closeDynamicPane(dynamic.paneIndex);
    await load;
    pending.resolve([status("too-late")]);

    expect(useAppStore.getState().dynamicColumns).toEqual([]);
    expect(useAppStore.getState().timelines.dynamic).toBeUndefined();
  });

  it("switches the acting account without clearing or reloading timeline state", async () => {
    const home = {
      ...column("home", null),
      columnType: "home",
      paneIndex: 0,
    };
    const publicTimeline = {
      ...column("public", null),
      columnType: "public",
      paneIndex: 1,
    };
    const list = {
      ...column("list", "17"),
      columnType: "list",
      accountAcct: "old@example.com",
      paneIndex: 2,
    };
    const columns = [home, publicTimeline, list];
    resetStore(columns);
    useAppStore.getState().replaceTimelineSlice(home.id, [status("home")], 100);
    useAppStore
      .getState()
      .replaceTimelineSlice(publicTimeline.id, [status("public")], 100);
    useAppStore.getState().replaceTimelineSlice(list.id, [status("list")], 100);
    useAppStore.setState({
      timelineUnread: { home: 3, public: 2, list: 1 },
      loading: { home: true },
      loadingMore: { list: true },
      timelineHasMore: { home: true, public: false, list: true },
      timelineNearTop: { home: false, public: true, list: false },
      activeTabs: { 0: home.id, 1: publicTimeline.id, 2: list.id },
    });
    const switched = {
      ...snapshot(columns),
      activeAcct: "new@example.com",
      accounts: [account("old@example.com", false), account("new@example.com", true)],
    };
    api.invokeCommand.mockResolvedValueOnce(switched);
    const before = useAppStore.getState();

    await useAppStore.getState().switchAccount("new@example.com");

    const state = useAppStore.getState();
    expect(api.invokeCommand).toHaveBeenCalledWith("switch_active_account", {
      acct: "new@example.com",
    });
    expect(api.invokeReadCommand).not.toHaveBeenCalled();
    expect(state.snapshot?.activeAcct).toBe("new@example.com");
    expect(state.snapshot?.accounts).toEqual(switched.accounts);
    expect(state.timelines).toBe(before.timelines);
    expect(state.timelineUnread).toEqual(before.timelineUnread);
    expect(state.loading).toEqual(before.loading);
    expect(state.loadingMore).toEqual(before.loadingMore);
    expect(state.timelineHasMore).toEqual(before.timelineHasMore);
    expect(state.timelineNearTop).toEqual(before.timelineNearTop);
    expect(state.activeTabs).toEqual(before.activeTabs);
  });
});

function resetStore(
  columns: ColumnSummary[],
  dynamicColumns: ColumnSummary[] = [],
) {
  const entities = createTimelineEntityState();
  useAppStore.setState({
    snapshot: snapshot(columns),
    entities: entities.entities,
    timelineKeys: entities.columnKeys,
    canonicalIndex: entities.canonicalIndex,
    timelines: entities.timelines,
    dynamicColumns,
    activeTabs: Object.fromEntries(
      [...columns, ...dynamicColumns].map((item) => [item.paneIndex, item.id]),
    ),
    loading: {},
    loadingMore: {},
    timelineHasMore: {},
    timelineNearTop: {},
    timelineUnread: {},
    resourceStates: {},
    statusMutations: {},
    mutationStates: {},
    error: undefined,
  });
}

function column(id: string, columnParam: string | null): ColumnSummary {
  return {
    id,
    paneIndex: id === "dynamic" ? 9 : 0,
    position: 0,
    columnType: "search",
    columnParam,
    name: id,
    maxStatuses: 100,
  };
}

function snapshot(columns: ColumnSummary[]): AppSnapshot {
  return {
    version: "test",
    accounts: [],
    columns,
    settings: {
      appearance: {
        avatar_shape: "Rounded",
        font_size: "Medium",
        cw_behavior: "Hide",
        nsfw_behavior: "Hide",
        display_mode: "StarryEyes",
      },
      performance: {
        mention_source: "Server",
        hashtag_source: "Server",
        timeline_renderer: "List",
      },
      confirmation: {
        confirm_boost: false,
        confirm_favourite: false,
        confirm_follow: false,
        confirm_unfollow: false,
        media_source: "Local",
        translate_enabled: false,
        auto_translate_enabled: false,
        translation_engine: "FoundationModel",
      },
      blueskyFetch: {},
      sidecars: { entries: [], mainViewIndex: 0 },
      accountSourceColors: {},
      presetVisibility: { entries: [] },
      debug: { logging_enabled: false, log_level: "Info" },
      notificationSuppression: { suppressed_accts: [] },
    },
    database: {
      path: "test.db",
      size: "0 B",
      statusCount: 0,
      recentStatusCount: 0,
      accountCount: 0,
    },
  };
}

function status(id: string): TimelineStatus {
  return {
    id,
    originalStatusId: id,
    statusIdentity: {
      protocol: "activityPub",
      serverDomain: "example.com",
      canonicalUri: `https://example.com/statuses/${id}`,
      remoteId: id,
    },
    sourceAcct: "user@example.com",
    accountId: "account",
    serverDomain: "example.com",
    uri: `https://example.com/statuses/${id}`,
    url: `https://example.com/statuses/${id}`,
    displayName: "User",
    acct: "user@example.com",
    avatar: "",
    createdAt: new Date(1_000_000).toISOString(),
    content: `<p>${id}</p>`,
    spoilerText: "",
    reblogsCount: 0,
    favouritesCount: 0,
    repliesCount: 0,
    visibility: "public",
    sensitive: false,
    favourited: false,
    reblogged: false,
    bookmarked: false,
    media: [],
    emojis: [],
    accountEmojis: [],
  };
}

function account(acct: string, isActive: boolean) {
  return {
    acct,
    serverDomain: "example.com",
    accountId: acct,
    displayName: acct,
    avatar: "",
    isActive,
    serverKind: "mastodon",
    characterLimit: 500,
    capabilities: {
      protocol: "activityPub" as const,
      timelines: {
        home: true,
        public: true,
        local: true,
        lists: true,
        hashtags: true,
        notifications: true,
        bookmarks: true,
        favourites: true,
      },
      status: {
        favourite: true,
        reblog: true,
        bookmark: true,
        vote: true,
        edit: true,
        delete: true,
      },
      relationship: { follow: true, mute: true, block: true },
      compose: {
        mediaUpload: true,
        poll: true,
        quote: true,
        maxMediaAttachments: 4,
        maxCharacters: 500,
      },
      streaming: true,
    },
  };
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((next) => {
    resolve = next;
  });
  return { promise, resolve };
}
