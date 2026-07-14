import { beforeEach, describe, expect, it, vi } from "vitest";
import type {
  AppSnapshot,
  ColumnSummary,
  TimelineStatus,
} from "../types/app";
import { createTimelineEntityState } from "../domain/timelineEntities";
import { frontendRequestScheduler } from "../utils/requestScheduler";
import { resetTimelineQueryCoordinator } from "./actions/timelineQueryActions";

const api = vi.hoisted(() => ({
  invokeCommand: vi.fn(),
  invokeCommandWithOperationId: vi.fn(),
  invokeTypedCommandWithOperationId: vi.fn(),
  invokeReadCommand: vi.fn(),
  invokeTypedReadCommand: vi.fn(),
  invokeTypedReadCommandWithOperationId: vi.fn(),
  invokeTypedCommand: vi.fn(),
}));

vi.mock("../api/tauri", () => api);

import { useAppStore } from "./appStore";

describe("appStore async resource generations", () => {
  beforeEach(() => {
    api.invokeCommand.mockReset();
    api.invokeCommandWithOperationId.mockReset();
    api.invokeTypedCommandWithOperationId.mockReset();
    api.invokeTypedCommandWithOperationId.mockImplementation((command, args) =>
      api.invokeTypedCommand(command, args),
    );
    api.invokeReadCommand.mockReset();
    api.invokeTypedReadCommand.mockReset();
    api.invokeTypedReadCommand.mockImplementation((command, args) =>
      api.invokeReadCommand(command, args),
    );
    api.invokeTypedReadCommandWithOperationId.mockReset();
    api.invokeTypedReadCommandWithOperationId.mockImplementation((command, args) =>
      api.invokeReadCommand(command, args),
    );
    api.invokeTypedCommand.mockReset();
    api.invokeTypedCommand.mockResolvedValue(true);
    api.invokeReadCommand.mockResolvedValue([]);
    frontendRequestScheduler.resetForTest();
    resetTimelineQueryCoordinator();
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
    expect(api.invokeReadCommand).toHaveBeenCalledWith(
      "load_timeline",
      expect.objectContaining({
        request: expect.objectContaining({ quoteConsumerId: dynamic.id }),
      }),
    );
    useAppStore.getState().closeDynamicPane(dynamic.paneIndex);
    await load;
    pending.resolve([status("too-late")]);

    expect(useAppStore.getState().dynamicColumns).toEqual([]);
    expect(useAppStore.getState().timelines.dynamic).toBeUndefined();
    expect(api.invokeTypedCommand).toHaveBeenCalledWith(
      "cancel_timeline_query",
      {
        request: {
          targetOperationId: expect.stringMatching(/^[0-9a-f-]{36}$/),
        },
      },
    );
    expect(api.invokeTypedCommand).toHaveBeenCalledWith(
      "cancel_quote_consumer",
      { request: { quoteConsumerId: dynamic.id } },
    );
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
    api.invokeTypedCommand.mockResolvedValueOnce(switched);
    api.invokeReadCommand.mockImplementationOnce((command, args) => {
      expect(command).toBe("status_viewer_states");
      const request = args?.request as {
        actingAccountAcct: string;
        identities: TimelineStatus["statusIdentity"][];
      };
      expect(request.actingAccountAcct).toBe("new@example.com");
      return Promise.resolve(
        request.identities.map((identity) => ({
          identity,
          favourited: identity.remoteId === "home",
          reblogged: identity.remoteId === "public",
          bookmarked: identity.remoteId === "list",
        })),
      );
    });
    const before = useAppStore.getState();

    await useAppStore.getState().switchAccount("new@example.com");

    const state = useAppStore.getState();
    expect(api.invokeTypedCommand).toHaveBeenCalledWith("switch_active_account", {
      acct: "new@example.com",
    });
    expect(api.invokeReadCommand).toHaveBeenCalledTimes(1);
    expect(state.snapshot?.activeAcct).toBe("new@example.com");
    expect(state.snapshot?.accounts).toEqual(switched.accounts);
    expect(Object.keys(state.timelines)).toEqual(Object.keys(before.timelines));
    expect(state.timelines.home.map((item) => item.id)).toEqual(["home"]);
    expect(state.timelines.public.map((item) => item.id)).toEqual(["public"]);
    expect(state.timelines.list.map((item) => item.id)).toEqual(["list"]);
    expect(state.timelines.home[0].favourited).toBe(true);
    expect(state.timelines.public[0].reblogged).toBe(true);
    expect(state.timelines.list[0].bookmarked).toBe(true);
    expect(state.timelineUnread).toEqual(before.timelineUnread);
    expect(state.loading).toEqual(before.loading);
    expect(state.loadingMore).toEqual(before.loadingMore);
    expect(state.timelineHasMore).toEqual(before.timelineHasMore);
    expect(state.timelineNearTop).toEqual(before.timelineNearTop);
    expect(state.activeTabs).toEqual(before.activeTabs);
  });

  it("keeps account switching serialized until viewer flags are reconciled", async () => {
    const home = { ...column("home", null), columnType: "home" };
    resetStore([home]);
    useAppStore.getState().replaceTimelineSlice(home.id, [status("home")], 100);
    const switched = {
      ...snapshot([home]),
      activeAcct: "new@example.com",
      accounts: [account("old@example.com", false), account("new@example.com", true)],
    };
    const viewerStates = deferred<unknown[]>();
    api.invokeTypedCommand.mockResolvedValueOnce(switched);
    api.invokeReadCommand.mockReturnValueOnce(viewerStates.promise);

    const first = useAppStore.getState().switchAccount("new@example.com");
    await vi.waitFor(() => expect(api.invokeReadCommand).toHaveBeenCalledTimes(1));
    const duplicate = useAppStore.getState().switchAccount("new@example.com");
    viewerStates.resolve([]);
    await Promise.all([first, duplicate]);

    expect(api.invokeTypedCommand).toHaveBeenCalledTimes(1);
    expect(useAppStore.getState().snapshot?.activeAcct).toBe("new@example.com");
  });

  it("does not retain the previous actor's viewer flags when reconciliation fails", async () => {
    const home = { ...column("home", null), columnType: "home" };
    resetStore([home]);
    useAppStore.getState().replaceTimelineSlice(
      home.id,
      [
        {
          ...status("home"),
          favourited: true,
          reblogged: true,
          bookmarked: true,
        },
      ],
      100,
    );
    api.invokeTypedCommand.mockResolvedValueOnce({
      ...snapshot([home]),
      activeAcct: "new@example.com",
      accounts: [account("old@example.com", false), account("new@example.com", true)],
    });
    api.invokeReadCommand.mockRejectedValueOnce(new Error("viewer state failed"));

    await useAppStore.getState().switchAccount("new@example.com");

    const state = useAppStore.getState();
    expect(state.snapshot?.activeAcct).toBe("new@example.com");
    expect(state.timelines.home[0]).toMatchObject({
      favourited: false,
      reblogged: false,
      bookmarked: false,
    });
    expect(state.error).toContain("viewer state failed");
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
