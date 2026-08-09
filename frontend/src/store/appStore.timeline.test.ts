import { beforeEach, describe, expect, it, vi } from "vitest";
import type {
  AppSnapshot,
  ColumnSummary,
  ComposeOutboxItem,
  TimelineStatus,
  TimelineStreamEvent,
} from "../types/app";
import {
  createTimelineEntityState,
  reduceTimelineEntities,
} from "../domain/timelineEntities";
import { IpcAppError } from "../api/ipcErrors";

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

import {
  flushAnalyticalTimelineRefreshesForTest,
  flushTimelineStreamEventsForTest,
  useAppStore,
} from "./appStore";
import { resetAnalyticalTimelineRefreshes } from "./actions/timelineStreamActions";
import { resetTimelineQueryCoordinator } from "./actions/timelineQueryActions";

const originalRequestConfirmation =
  useAppStore.getState().requestConfirmation;

describe("appStore normalized status mutation pipeline", () => {
  const home = fixtureColumn("home", "home", 5);
  const status = fixtureStatus("1");

  beforeEach(() => {
    resetAnalyticalTimelineRefreshes();
    resetTimelineQueryCoordinator();
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
    api.invokeReadCommand.mockImplementation((command) =>
      command === "load_more_timeline"
          ? Promise.resolve({ statuses: [], hasMore: false })
          : Promise.resolve([]),
    );
    resetTimelineStore([home], { home: [status] });
  });

  it("rolls back an optimistic action after a confirmed failure", async () => {
    api.invokeTypedCommand.mockRejectedValueOnce(new Error("permission denied"));

    await useAppStore
      .getState()
      .actionStatus(status, "favourite", false);

    expect(useAppStore.getState().timelines.home[0].favourited).toBe(false);
    expect(
      Object.values(useAppStore.getState().statusMutations)[0].phase,
    ).toBe("failed");
  });

  it("deduplicates status actions while confirmation is open", async () => {
    let resolveConfirmation: ((confirmed: boolean) => void) | undefined;
    const requestConfirmation = vi.fn(
      () =>
        new Promise<boolean>((resolve) => {
          resolveConfirmation = resolve;
        }),
    );
    useAppStore.setState((state) => ({
      snapshot: state.snapshot
        ? {
            ...state.snapshot,
            settings: {
              ...state.snapshot.settings,
              confirmation: {
                ...state.snapshot.settings.confirmation,
                confirm_favourite: true,
              },
            },
          }
        : state.snapshot,
      requestConfirmation,
    }));
    api.invokeTypedCommand.mockResolvedValueOnce({ ...status, favourited: true });

    const first = useAppStore.getState().actionStatus(status, "favourite");
    const duplicate = useAppStore.getState().actionStatus(status, "favourite");
    await vi.waitFor(() => expect(requestConfirmation).toHaveBeenCalledTimes(1));
    resolveConfirmation?.(true);
    await Promise.all([first, duplicate]);

    expect(api.invokeTypedCommand).toHaveBeenCalledTimes(1);
  });

  it("keeps the captured acting account and canonical identity across an account switch", async () => {
    let resolveConfirmation: ((confirmed: boolean) => void) | undefined;
    useAppStore.setState((state) => ({
      snapshot: state.snapshot
        ? {
            ...state.snapshot,
            settings: {
              ...state.snapshot.settings,
              confirmation: {
                ...state.snapshot.settings.confirmation,
                confirm_favourite: true,
              },
            },
          }
        : state.snapshot,
      requestConfirmation: () =>
        new Promise<boolean>((resolve) => {
          resolveConfirmation = resolve;
        }),
    }));
    api.invokeTypedCommand.mockResolvedValueOnce({ ...status, favourited: true });

    const action = useAppStore
      .getState()
      .actionStatus(status, "favourite", true);
    useAppStore.setState((state) => ({
      snapshot: state.snapshot
        ? { ...state.snapshot, activeAcct: "bob@alpha.example" }
        : state.snapshot,
    }));
    await vi.waitFor(() => expect(resolveConfirmation).toBeDefined());
    resolveConfirmation?.(true);
    await action;

    expect(api.invokeTypedCommand).toHaveBeenCalledWith("status_action", {
      request: {
        actingAccountAcct: "user@alpha.example",
        action: "favourite",
        identity: status.statusIdentity,
      },
    });
  });

  it("abandons a status action whose viewer-state read is invalidated by account switching", async () => {
    let releaseViewerState:
      | ((value: Array<{
          identity: TimelineStatus["statusIdentity"];
          favourited: boolean;
          reblogged: boolean;
          bookmarked: boolean;
        }>) => void)
      | undefined;
    api.invokeReadCommand.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          releaseViewerState = resolve;
        }),
    );
    const currentSnapshot = useAppStore.getState().snapshot!;
    const currentAccount = currentSnapshot.accounts[0]!;
    const switchedSnapshot: AppSnapshot = {
      ...currentSnapshot,
      activeAcct: "bob@alpha.example",
      accounts: [
        { ...currentAccount, isActive: false },
        {
          ...currentAccount,
          acct: "bob@alpha.example",
          accountId: "account-bob",
          displayName: "Bob",
          isActive: true,
        },
      ],
    };
    api.invokeTypedCommand.mockImplementation((command) =>
      command === "switch_active_account"
        ? Promise.resolve(switchedSnapshot)
        : Promise.resolve(true),
    );

    const action = useAppStore.getState().actionStatus(status, "favourite", false);
    await vi.waitFor(() => expect(releaseViewerState).toBeDefined());
    await useAppStore.getState().switchAccount("bob@alpha.example");
    releaseViewerState?.([
      {
        identity: status.statusIdentity,
        favourited: true,
        reblogged: false,
        bookmarked: false,
      },
    ]);
    await action;

    expect(useAppStore.getState().snapshot?.activeAcct).toBe("bob@alpha.example");
    expect(
      api.invokeTypedCommand.mock.calls.filter(
        ([command]) => command === "status_action",
      ),
    ).toHaveLength(0);
    expect(useAppStore.getState().timelines.home[0]?.favourited).toBe(false);
    expect(Object.keys(useAppStore.getState().statusMutations)).toHaveLength(0);
  });

  it("does not start actor-scoped mutations while an account switch is waiting for IPC", async () => {
    let releaseSwitch: ((value: AppSnapshot) => void) | undefined;
    const currentSnapshot = useAppStore.getState().snapshot!;
    const currentAccount = currentSnapshot.accounts[0]!;
    const switchedSnapshot: AppSnapshot = {
      ...currentSnapshot,
      activeAcct: "bob@alpha.example",
      accounts: [
        { ...currentAccount, isActive: false },
        {
          ...currentAccount,
          acct: "bob@alpha.example",
          accountId: "account-bob",
          displayName: "Bob",
          isActive: true,
        },
      ],
    };
    api.invokeTypedCommand.mockImplementation((command) => {
      if (command === "switch_active_account") {
        return new Promise((resolve) => {
          releaseSwitch = resolve;
        });
      }
      return Promise.resolve(true);
    });

    const switching = useAppStore.getState().switchAccount("bob@alpha.example");
    await vi.waitFor(() => expect(releaseSwitch).toBeDefined());
    await useAppStore.getState().actionStatus(status, "favourite", false);
    useAppStore.setState({
      composeText: "reply during switch",
      composeTarget: {
        kind: "reply",
        status,
        visibilityBeforeReply: "public",
      },
    });
    expect(await useAppStore.getState().post()).toBe(false);

    expect(api.invokeReadCommand).not.toHaveBeenCalled();
    expect(
      api.invokeTypedCommand.mock.calls.filter(
        ([command]) => command === "status_action",
      ),
    ).toHaveLength(0);
    expect(
      api.invokeTypedCommand.mock.calls.filter(
        ([command]) => command === "enqueue_post_status",
      ),
    ).toHaveLength(0);
    expect(Object.keys(useAppStore.getState().statusMutations)).toHaveLength(0);

    releaseSwitch?.(switchedSnapshot);
    await switching;
    expect(useAppStore.getState().snapshot?.activeAcct).toBe("bob@alpha.example");
  });

  it("does not let delayed account reconciliation overwrite a newer status action", async () => {
    let releaseReconciliation:
      | ((value: Array<{
          identity: TimelineStatus["statusIdentity"];
          favourited: boolean;
          reblogged: boolean;
          bookmarked: boolean;
        }>) => void)
      | undefined;
    api.invokeReadCommand
      .mockImplementationOnce(
        () =>
          new Promise((resolve) => {
            releaseReconciliation = resolve;
          }),
      )
      .mockResolvedValueOnce([
        {
          identity: status.statusIdentity,
          favourited: false,
          reblogged: false,
          bookmarked: false,
        },
      ]);
    const currentSnapshot = useAppStore.getState().snapshot!;
    const currentAccount = currentSnapshot.accounts[0]!;
    const switchedSnapshot: AppSnapshot = {
      ...currentSnapshot,
      activeAcct: "bob@alpha.example",
      accounts: [
        { ...currentAccount, isActive: false },
        {
          ...currentAccount,
          acct: "bob@alpha.example",
          accountId: "account-bob",
          displayName: "Bob",
          isActive: true,
        },
      ],
    };
    api.invokeTypedCommand.mockImplementation((command) => {
      if (command === "switch_active_account") return Promise.resolve(switchedSnapshot);
      if (command === "status_action") {
        return Promise.resolve({ ...status, favourited: true });
      }
      return Promise.resolve(true);
    });

    const switching = useAppStore.getState().switchAccount("bob@alpha.example");
    await vi.waitFor(() => {
      expect(useAppStore.getState().snapshot?.activeAcct).toBe("bob@alpha.example");
      expect(releaseReconciliation).toBeDefined();
    });
    await useAppStore.getState().actionStatus(status, "favourite", false);
    expect(useAppStore.getState().timelines.home[0]?.favourited).toBe(true);

    releaseReconciliation?.([
      {
        identity: status.statusIdentity,
        favourited: false,
        reblogged: false,
        bookmarked: false,
      },
    ]);
    await switching;

    expect(useAppStore.getState().timelines.home[0]?.favourited).toBe(true);
  });

  it("rolls back an optimistic status action when account switching invalidates it", async () => {
    let releaseAction: ((value: TimelineStatus) => void) | undefined;
    const currentSnapshot = useAppStore.getState().snapshot!;
    api.invokeTypedCommand.mockImplementation((command) => {
      if (command === "status_action") {
        return new Promise((resolve) => {
          releaseAction = resolve;
        });
      }
      if (command === "switch_active_account") {
        return Promise.reject(new Error("switch failed"));
      }
      return Promise.resolve(true);
    });

    const action = useAppStore.getState().actionStatus(status, "favourite", false);
    await vi.waitFor(() => {
      expect(releaseAction).toBeDefined();
      expect(useAppStore.getState().timelines.home[0]?.favourited).toBe(true);
    });
    await useAppStore.getState().switchAccount("bob@alpha.example");
    expect(useAppStore.getState().snapshot).toBe(currentSnapshot);
    expect(useAppStore.getState().timelines.home[0]?.favourited).toBe(false);

    releaseAction?.({ ...status, favourited: true });
    await action;

    expect(useAppStore.getState().timelines.home[0]?.favourited).toBe(false);
    expect(Object.keys(useAppStore.getState().statusMutations)).toHaveLength(0);
  });

  it.each([
    ["unfavourite", "favourited"],
    ["unreblog", "reblogged"],
    ["unbookmark", "bookmarked"],
  ] as const)(
    "reconciles the selected account before a cross-source %s action",
    async (action, viewerFlag) => {
      const remoteCopy = fixtureStatus("remote-copy", {
        sourceAcct: "bob@misskey.example",
        serverDomain: "misskey.example",
        uri: "https://origin.example/users/alice/statuses/1",
        [viewerFlag]: true,
        statusIdentity: {
          protocol: "activityPub",
          serverDomain: "misskey.example",
          canonicalUri: "https://origin.example/users/alice/statuses/1",
          remoteId: "misskey-local-id",
        },
      });
      resetTimelineStore([home], { home: [remoteCopy] });
      api.invokeReadCommand.mockResolvedValueOnce([
        {
          identity: remoteCopy.statusIdentity,
          favourited: false,
          reblogged: false,
          bookmarked: false,
        },
      ]);

      await useAppStore.getState().actionStatus(remoteCopy, action, false);

      expect(api.invokeReadCommand).toHaveBeenCalledWith("status_viewer_states", {
        request: {
          actingAccountAcct: "user@alpha.example",
          identities: [remoteCopy.statusIdentity],
        },
      });
      expect(api.invokeTypedCommand).not.toHaveBeenCalled();
      expect(useAppStore.getState().timelines.home[0]?.[viewerFlag]).toBe(false);
    },
  );

  it("reconciles viewer state even when the timeline source matches the selected account", async () => {
    api.invokeReadCommand.mockResolvedValueOnce([
      {
        identity: status.statusIdentity,
        favourited: true,
        reblogged: false,
        bookmarked: false,
      },
    ]);

    await useAppStore.getState().actionStatus(status, "favourite", false);

    expect(api.invokeReadCommand).toHaveBeenCalledWith("status_viewer_states", {
      request: {
        actingAccountAcct: "user@alpha.example",
        identities: [status.statusIdentity],
      },
    });
    expect(api.invokeTypedCommand).not.toHaveBeenCalled();
    expect(useAppStore.getState().timelines.home[0]?.favourited).toBe(true);
    expect(Object.values(useAppStore.getState().statusMutations)[0]?.phase).toBe(
      "confirmed",
    );
  });

  it("deduplicates compose submit double clicks", async () => {
    let release: ((item: ComposeOutboxItem) => void) | undefined;
    api.invokeTypedCommand.mockImplementationOnce(
      () =>
        new Promise<ComposeOutboxItem>((resolve) => {
          release = resolve;
        }),
    );
    useAppStore.setState({ composeText: "hello", composeTarget: null });

    const first = useAppStore.getState().post();
    const duplicate = useAppStore.getState().post();
    release?.(fixtureOutboxItem());
    await Promise.all([first, duplicate]);

    expect(api.invokeTypedCommand).toHaveBeenCalledTimes(1);
  });

  it("keeps the original visibility when submitting an edited post", async () => {
    const edited = fixtureStatus("edit-1", {
      content: "<p>notification</p>",
      visibility: "private",
    });
    useAppStore.setState((state) => ({
      snapshot: state.snapshot
        ? {
            ...state.snapshot,
            settings: {
              ...state.snapshot.settings,
              presetVisibility: {
                entries: [{ keyword: "notification", visibility: "Public" }],
              },
            },
          }
        : state.snapshot,
    }));
    useAppStore.getState().beginEditStatus(edited);
    useAppStore.setState({
      composeText: "notification revised",
      visibility: "public",
    });

    expect(await useAppStore.getState().post()).toBe(true);

    expect(api.invokeTypedCommandWithOperationId).toHaveBeenCalledWith(
      "enqueue_edit_status",
      {
        request: expect.objectContaining({
          identity: edited.statusIdentity,
          visibility: "private",
        }),
      },
      expect.any(String),
    );
  });

  it("submits a reply identity for resolution on the selected account server", async () => {
    const remoteCopy = fixtureStatus("misskey-local-id", {
      serverDomain: "misskey.example",
      visibility: "unlisted",
      uri: "https://origin.example/users/alice/statuses/1",
      statusIdentity: {
        protocol: "activityPub",
        serverDomain: "misskey.example",
        canonicalUri: "https://origin.example/users/alice/statuses/1",
        remoteId: "misskey-local-id",
      },
    });
    useAppStore.setState({
      composeText: "",
      composeTarget: null,
      visibility: "private",
    });
    useAppStore.getState().replyStatus(remoteCopy);
    useAppStore.setState({ composeText: "reply" });
    api.invokeTypedCommand.mockResolvedValueOnce(fixtureOutboxItem());

    expect(await useAppStore.getState().post()).toBe(true);

    const request = api.invokeTypedCommand.mock.calls[0]?.[1]?.request;
    expect(request).toMatchObject({
      actingAccountAcct: "user@alpha.example",
      visibility: "unlisted",
      inReplyToIdentity: remoteCopy.statusIdentity,
    });
    expect(request?.inReplyToId).toBeUndefined();
    expect(useAppStore.getState().visibility).toBe("private");
  });

  it("releases compose after enqueue and inserts the worker result into every unified Home column", async () => {
    const mastodonHome = {
      ...fixtureColumn("post-mastodon-home", "home", 5),
      accountAcct: "alice@mastodon.example",
    };
    const blueskyHome = {
      ...fixtureColumn("post-bluesky-home", "home", 5),
      accountAcct: "alice.bsky@bsky.social",
    };
    const posted = fixtureStatus("posted", {
      sourceAcct: "user@alpha.example",
    });
    resetTimelineStore([mastodonHome, blueskyHome], {});
    useAppStore.setState({ composeText: "hello", composeTarget: null });
    const queued = fixtureOutboxItem();
    api.invokeTypedCommand.mockResolvedValueOnce(queued);

    expect(await useAppStore.getState().post()).toBe(true);
    expect(useAppStore.getState().timelines[mastodonHome.id]).toEqual([]);
    expect(useAppStore.getState().composeOutboxItems).toEqual([queued]);
    useAppStore.getState().applyComposeOutboxUpdate({
      item: { ...queued, state: "succeeded" },
      status: posted,
    });
    flushTimelineStreamEventsForTest();

    const timelines = useAppStore.getState().timelines;
    expect(timelines[mastodonHome.id]).toEqual([posted]);
    expect(timelines[blueskyHome.id]).toEqual([posted]);
    expect(useAppStore.getState().composeText).toBe("");
    expect(useAppStore.getState().mutationStates["compose:submit"]?.phase).toBe(
      "succeeded",
    );
    expect(api.invokeReadCommand).not.toHaveBeenCalled();
  });

  it("uses the selected account only as a unified timeline ranking hint", async () => {
    const home = {
      ...fixtureColumn("request-home", "home", 5),
      accountAcct: "legacy@mastodon.example",
    };
    const notification = {
      ...fixtureColumn("request-notification", "notification", 5),
      accountAcct: "legacy@mastodon.example",
    };
    const custom = {
      ...fixtureColumn("request-custom", "custom", 5),
      accountAcct: "legacy@mastodon.example",
      columnParam: "SELECT * FROM timeline_statuses",
    };
    const list = {
      ...fixtureColumn("request-list", "list", 5),
      accountAcct: "alice@mastodon.example",
      columnParam: "17",
    };
    resetTimelineStore([home, notification, custom, list], {});

    await useAppStore.getState().loadTimeline(home);
    await useAppStore.getState().loadTimeline(notification, true);
    await useAppStore.getState().loadTimeline(custom);
    await useAppStore.getState().loadTimeline(list);

    const timelineRequests = api.invokeReadCommand.mock.calls.map(
      ([command, args]) => ({ command, request: args.request }),
    );
    expect(timelineRequests[0]).toMatchObject({ command: "load_timeline" });
    expect(timelineRequests[0]?.request).not.toHaveProperty("accountAcct");
    expect(timelineRequests[0]?.request).toMatchObject({
      actingAccountAcct: "user@alpha.example",
    });
    expect(timelineRequests[1]).toMatchObject({ command: "refresh_timeline" });
    expect(timelineRequests[1]?.request).not.toHaveProperty("accountAcct");
    expect(timelineRequests[1]?.request).toMatchObject({
      actingAccountAcct: "user@alpha.example",
    });
    expect(timelineRequests[2]).toMatchObject({ command: "load_timeline" });
    expect(timelineRequests[2]?.request).not.toHaveProperty("accountAcct");
    expect(timelineRequests[2]?.request).not.toHaveProperty("actingAccountAcct");
    expect(timelineRequests[3]).toMatchObject({
      command: "load_timeline",
      request: { accountAcct: "alice@mastodon.example" },
    });
  });

  it("stores reviewed custom timeline copy without the IPC error class name", async () => {
    const custom = {
      ...fixtureColumn("fts-error-copy", "custom", 5),
      columnParam: "SELECT * FROM status_search_icu_fts",
    };
    resetTimelineStore([custom], {});
    api.invokeReadCommand.mockRejectedValueOnce(
      new IpcAppError({
        code: "internal",
        messageKey: "errors.custom_timeline_fts_match_or",
        retryable: false,
        requestId: "11111111-1111-4111-8111-111111111111",
      }),
    );

    await useAppStore.getState().loadTimeline(custom);

    const message = useAppStore.getState().resourceStates[
      `timeline:${custom.id}`
    ]?.error;
    expect(message).toBe(
      "FTS search conditions are invalid. Combine alternatives inside one MATCH expression with OR.",
    );
    expect(message).not.toContain("IpcAppError");
  });

  it("loads analytical columns concurrently with one lane slot per column", async () => {
    const customA = {
      ...fixtureColumn("analytics-a", "custom", 5),
      columnParam: "SELECT * FROM timeline_statuses",
    };
    const customB = {
      ...fixtureColumn("analytics-b", "custom", 5),
      columnParam: "SELECT * FROM timeline_statuses",
    };
    resetTimelineStore([customA, customB], {});
    const releases: Array<(statuses: TimelineStatus[]) => void> = [];
    api.invokeReadCommand.mockImplementation(
      () =>
        new Promise<TimelineStatus[]>((resolve) => {
          releases.push(resolve);
        }),
    );

    const loads = Promise.all([
      useAppStore.getState().loadTimeline(customA),
      useAppStore.getState().loadTimeline(customB),
    ]);
    // Both analytical reads must be in flight at once; the second column no
    // longer waits behind the first one.
    await vi.waitFor(() =>
      expect(api.invokeReadCommand).toHaveBeenCalledTimes(2),
    );

    for (const release of releases) release([]);
    await loads;
    expect(useAppStore.getState().loading[customA.id]).toBe(false);
    expect(useAppStore.getState().loading[customB.id]).toBe(false);
  });

  it("uses the selected account to rank unified pagination without filtering", async () => {
    const columns = ["home", "public", "notification"].map((columnType) => ({
      ...fixtureColumn(`more-${columnType}`, columnType, 20),
      accountAcct: `legacy-${columnType}@example.social`,
    }));
    const timelines = Object.fromEntries(
      columns.map((column, index) => [
        column.id,
        [fixtureStatus(`more-${index}`, { createdAt: new Date(index + 1).toISOString() })],
      ]),
    );
    resetTimelineStore(columns, timelines);
    api.invokeReadCommand.mockResolvedValue([]);

    for (const column of columns) {
      await useAppStore.getState().loadMoreTimeline(column);
    }

    const requests = api.invokeReadCommand.mock.calls.map(
      ([command, args]) => ({ command, request: args.request }),
    );
    expect(requests).toHaveLength(3);
    for (const request of requests) {
      expect(request.command).toBe("load_timeline");
      expect(request.request).toMatchObject({
        offset: 1,
        actingAccountAcct: "user@alpha.example",
      });
      expect(request.request).not.toHaveProperty("accountAcct");
    }
  });

  it("keeps API pagination available when a provider caps the initial page", async () => {
    const list = {
      ...fixtureColumn("capped-list", "list", 100),
      accountAcct: "alice@mastodon.example",
      columnParam: "14",
    };
    const initial = Array.from({ length: 40 }, (_, index) =>
      fixtureStatus(`initial-${index}`, {
        createdAt: new Date(100_000 - index * 1_000).toISOString(),
      }),
    );
    const older = fixtureStatus("older", {
      createdAt: new Date(50_000).toISOString(),
    });
    resetTimelineStore([list], {});
    api.invokeReadCommand.mockImplementation((command) => {
      if (command === "refresh_timeline") return Promise.resolve(initial);
      if (command === "load_more_timeline") {
        return Promise.resolve({ statuses: [older], hasMore: false });
      }
      return Promise.resolve([]);
    });

    await useAppStore.getState().loadTimeline(list, true);

    expect(useAppStore.getState().timelineHasMore[list.id]).toBe(true);

    await useAppStore.getState().loadMoreTimeline(list);

    expect(api.invokeReadCommand).toHaveBeenCalledWith("load_more_timeline", {
      request: expect.objectContaining({
        accountAcct: "alice@mastodon.example",
        columnParam: "14",
        maxStatusId: "initial-39",
      }),
    });
    expect(useAppStore.getState().timelines[list.id]).toHaveLength(41);
    expect(useAppStore.getState().timelines[list.id][40]?.id).toBe("older");
    expect(useAppStore.getState().timelineHasMore[list.id]).toBe(false);
  });

  it("advances local pagination without a global retention cap", async () => {
    const publicColumn = fixtureColumn("paged-public", "public", 100);
    const initial = Array.from({ length: 100 }, (_, index) =>
      fixtureStatus(`initial-${index}`, {
        createdAt: new Date(400_000 - index * 1_000).toISOString(),
      }),
    );
    const fullPages = Array.from({ length: 10 }, (_, page) =>
      Array.from({ length: 100 }, (_, index) =>
        fixtureStatus(`page-${page}-${index}`, {
          createdAt: new Date(
            300_000 - page * 100_000 - index * 1_000,
          ).toISOString(),
        }),
      ),
    );
    const finalPage = [fixtureStatus("final-page", {
      createdAt: new Date(-800_000).toISOString(),
    })];
    resetTimelineStore([publicColumn], { [publicColumn.id]: initial });
    for (const page of fullPages) {
      api.invokeReadCommand.mockResolvedValueOnce(page);
    }
    api.invokeReadCommand.mockResolvedValueOnce(finalPage);

    for (let page = 0; page < fullPages.length; page += 1) {
      await useAppStore.getState().loadMoreTimeline(publicColumn);
    }

    expect(useAppStore.getState().timelines[publicColumn.id]).toHaveLength(1_100);
    expect(api.invokeReadCommand.mock.calls[0]?.[1].request).toMatchObject({
      offset: 100,
    });
    expect(api.invokeReadCommand.mock.calls[9]?.[1].request).toMatchObject({
      offset: 1_000,
    });
    expect(useAppStore.getState().timelineHasMore[publicColumn.id]).toBe(true);

    await useAppStore.getState().loadMoreTimeline(publicColumn);

    expect(useAppStore.getState().timelines[publicColumn.id]).toHaveLength(1_101);
    expect(api.invokeReadCommand.mock.calls[10]?.[1].request).toMatchObject({
      offset: 1_100,
    });
    expect(useAppStore.getState().timelineHasMore[publicColumn.id]).toBe(false);

    useAppStore.setState({
      timelineNearTop: { [publicColumn.id]: false },
    });
    for (let index = 0; index < 3; index += 1) {
      const streamed = fixtureStatus(`streamed-after-pagination-${index}`, {
        createdAt: new Date(500_000 + index * 1_000).toISOString(),
      });
      useAppStore.getState().applyStreamEvent({
        ...streamEvent(streamed),
        streamType: "public",
      });
    }
    flushTimelineStreamEventsForTest();

    expect(useAppStore.getState().timelines[publicColumn.id]).toHaveLength(1_101);
    expect(
      useAppStore.getState().timelineDeferredKeys[publicColumn.id],
    ).toHaveLength(3);
    expect(
      useAppStore.getState().timelines[publicColumn.id].some(
        (status) => status.id === "page-9-99",
      ),
    ).toBe(true);

    api.invokeReadCommand.mockResolvedValueOnce(initial.slice(0, 80));
    await useAppStore.getState().loadTimeline(publicColumn, true);

    expect(useAppStore.getState().timelines[publicColumn.id]).toHaveLength(1_101);
    expect(
      useAppStore.getState().timelines[publicColumn.id].some(
        (status) => status.id === "page-9-99",
      ),
    ).toBe(true);

    useAppStore.getState().setTimelineNearTop(publicColumn, true);
    expect(useAppStore.getState().timelines[publicColumn.id]).toHaveLength(100);
    expect(useAppStore.getState().timelines[publicColumn.id][0]?.id).toBe(
      "streamed-after-pagination-2",
    );
    // The trim dropped explicitly loaded pages, so load-more must be
    // available again to page the history back in.
    expect(useAppStore.getState().timelineHasMore[publicColumn.id]).toBe(true);
  });

  it("caps an over-retained near-top column when a stream status arrives", async () => {
    const publicColumn = fixtureColumn("stream-cap-public", "public", 100);
    const initial = Array.from({ length: 100 }, (_, index) =>
      fixtureStatus(`cap-initial-${index}`, {
        createdAt: new Date(400_000 - index * 1_000).toISOString(),
      }),
    );
    const olderPage = Array.from({ length: 40 }, (_, index) =>
      fixtureStatus(`cap-older-${index}`, {
        createdAt: new Date(200_000 - index * 1_000).toISOString(),
      }),
    );
    resetTimelineStore([publicColumn], { [publicColumn.id]: initial });
    api.invokeReadCommand.mockResolvedValueOnce(olderPage);

    await useAppStore.getState().loadMoreTimeline(publicColumn);

    expect(useAppStore.getState().timelines[publicColumn.id]).toHaveLength(140);
    expect(useAppStore.getState().timelineHasMore[publicColumn.id]).toBe(false);

    useAppStore.getState().applyStreamEvent({
      ...streamEvent(
        fixtureStatus("cap-streamed", {
          createdAt: new Date(500_000).toISOString(),
        }),
      ),
      streamType: "public",
    });
    flushTimelineStreamEventsForTest();

    const state = useAppStore.getState();
    expect(state.timelines[publicColumn.id]).toHaveLength(100);
    expect(state.timelines[publicColumn.id][0]?.id).toBe("cap-streamed");
    expect(
      state.timelines[publicColumn.id].some(
        (status) => status.id === "cap-older-0",
      ),
    ).toBe(false);
    expect(state.timelineHasMore[publicColumn.id]).toBe(true);
  });

  it("stops automatic pagination when a page makes no retained progress", async () => {
    const publicColumn = fixtureColumn("duplicate-public", "public", 100);
    const initial = Array.from({ length: 100 }, (_, index) =>
      fixtureStatus(`duplicate-${index}`, {
        createdAt: new Date(200_000 - index * 1_000).toISOString(),
      }),
    );
    resetTimelineStore([publicColumn], { [publicColumn.id]: initial });
    api.invokeReadCommand.mockResolvedValue(initial);

    await useAppStore.getState().loadMoreTimeline(publicColumn);
    await useAppStore.getState().loadMoreTimeline(publicColumn);

    expect(api.invokeReadCommand).toHaveBeenCalledTimes(1);
    expect(useAppStore.getState().timelines[publicColumn.id]).toHaveLength(100);
    expect(useAppStore.getState().timelineHasMore[publicColumn.id]).toBe(false);
  });

  it("captures the originating session for profile, thread, and AIR reads", async () => {
    const source = fixtureStatus("read-source", {
      sourceAcct: "bob@alpha.example",
      notificationAccountId: "notification-actor",
      notificationAcct: "actor@alpha.example",
      createdAt: "2026-07-18T20:11:49Z",
      originalCreatedAt: "2026-07-18T20:11:30Z",
    });

    useAppStore.getState().openUserPane(source);
    useAppStore.getState().openThreadPane(source);
    useAppStore.getState().openAirContextPane(source);

    const columns = useAppStore.getState().dynamicColumns;
    expect(columns.find((column) => column.columnType === "profile")?.profile)
      .toMatchObject({ sourceAcct: "bob@alpha.example" });
    const thread = columns.find((column) => column.columnType === "thread");
    const air = columns.find((column) => column.columnType === "airContext");
    expect(thread).toBeDefined();
    expect(air).toBeDefined();

    await useAppStore.getState().loadTimeline(thread!);
    await useAppStore.getState().loadTimeline(air!);

    expect(api.invokeReadCommand).toHaveBeenCalledWith("status_thread", {
      request: expect.objectContaining({
        statusId: "read-source",
        serverDomain: "alpha.example",
        sourceAcct: "bob@alpha.example",
      }),
    });
    expect(api.invokeReadCommand).toHaveBeenCalledWith("air_context", {
      request: expect.objectContaining({
        statusId: "read-source",
        serverDomain: "alpha.example",
        sourceAcct: "bob@alpha.example",
        notificationCreatedAt: "2026-07-18T20:11:49Z",
      }),
    });
  });

  it("keeps the optimistic result and marks response loss as uncertain", async () => {
    api.invokeTypedCommand.mockRejectedValueOnce(
      new IpcAppError({
        code: "ipc_response_lost",
        messageKey: "errors.ipc_response_lost",
        retryable: true,
        requestId: "11111111-1111-4111-8111-111111111111",
      }),
    );

    await useAppStore
      .getState()
      .actionStatus(status, "favourite", false);

    const state = useAppStore.getState();
    expect(state.timelines.home[0].favourited).toBe(true);
    expect(Object.values(state.statusMutations)[0].phase).toBe("uncertain");
  });

  it("commits one returned entity to every column and the media overlay", async () => {
    const local = fixtureColumn("local", "local", 5);
    resetTimelineStore(
      [home, local],
      { home: [status], local: [{ ...status }] },
      {
        mediaPreview: {
          status,
          media: { id: "media-1", url: "https://alpha.example/media/1" },
          src: "https://alpha.example/media/1",
        },
      },
    );
    useAppStore.setState({ error: "unrelated profile request failed" });
    api.invokeTypedCommand.mockResolvedValueOnce({
      ...status,
      favourited: true,
      favouritesCount: 1,
    });

    await useAppStore
      .getState()
      .actionStatus(status, "favourite", false);

    const state = useAppStore.getState();
    expect(state.timelines.home[0]).toBe(state.timelines.local[0]);
    expect(state.mediaPreview?.status).toBe(state.timelines.home[0]);
    expect(state.timelines.home[0].favourited).toBe(true);
    expect(Object.values(state.statusMutations)[0].phase).toBe("confirmed");
    expect(state.error).toBe("unrelated profile request failed");
  });

  it("invalidates analytical timelines only after a mutation command commits", async () => {
    const custom = fixtureColumn("mutation-custom", "custom", 100);
    resetTimelineStore(
      [custom],
      { [custom.id]: [status] },
      { activeTabs: { 0: custom.id } },
    );
    api.invokeTypedCommand.mockResolvedValueOnce({
      ...status,
      favourited: true,
      favouritesCount: 1,
    });

    await useAppStore.getState().actionStatus(status, "favourite", false);
    expect(
      api.invokeReadCommand.mock.calls.filter(
        ([command]) => command === "refresh_timeline",
      ),
    ).toHaveLength(0);
    await flushAnalyticalTimelineRefreshesForTest();

    expect(
      api.invokeReadCommand.mock.calls.filter(
        ([command]) => command === "refresh_timeline",
      ),
    ).toHaveLength(1);
  });

  it("invalidates analytical timelines after a provider refresh commits", async () => {
    const custom = {
      ...fixtureColumn("refresh-custom", "yq", 100),
      paneIndex: 1,
    };
    resetTimelineStore(
      [home, custom],
      { [home.id]: [status], [custom.id]: [status] },
      { activeTabs: { 0: home.id, 1: custom.id } },
    );

    await useAppStore.getState().loadTimeline(home, true);
    expect(
      api.invokeReadCommand.mock.calls.filter(
        ([command]) => command === "refresh_timeline",
      ),
    ).toHaveLength(1);
    await flushAnalyticalTimelineRefreshesForTest();

    expect(
      api.invokeReadCommand.mock.calls.filter(
        ([command]) => command === "refresh_timeline",
      ),
    ).toHaveLength(2);
  });

  it("coalesces a burst into one measured batch and preserves a far anchor", () => {
    useAppStore.setState((state) => ({
      timelineNearTop: { ...state.timelineNearTop, home: false },
      timelineHasMore: { ...state.timelineHasMore, home: false },
    }));
    const anchor = [...useAppStore.getState().timelineKeys.home];
    for (let index = 0; index < 500; index += 1) {
      useAppStore.getState().applyStreamEvent(
        streamEvent(
          fixtureStatus("new", {
            content: `<p>version ${index}</p>`,
            createdAt: new Date(2_000_000).toISOString(),
          }),
        ),
      );
    }

    flushTimelineStreamEventsForTest();

    const state = useAppStore.getState();
    expect(state.timelineKeys.home).toEqual(anchor);
    expect(state.timelineUnread.home).toBe(1);
    expect(state.timelineHasMore.home).toBe(false);
    expect(state.streamPerformance.lastBatchSize).toBe(1);
    expect(state.streamPerformance.p95DurationMs).toBeGreaterThanOrEqual(0);
  });

  it("counts only URI-unique deferred rows newer than the visible timeline head", () => {
    const federated = fixtureColumn("federated-unread", "public", 100);
    const visibleHead = fixtureStatus("visible-head", {
      createdAt: "2026-07-17T15:24:30.000Z",
    });
    resetTimelineStore(
      [federated],
      { [federated.id]: [visibleHead] },
      { timelineNearTop: { [federated.id]: false } },
    );

    const delayedOlder = fixtureStatus("delayed-older", {
      uri: "https://remote.example/@user/older",
      createdAt: "2026-07-17T15:23:00.000Z",
    });
    const genuinelyNew = fixtureStatus("genuinely-new", {
      uri: "https://remote.example/@user/new",
      createdAt: "2026-07-17T15:25:00.000Z",
    });
    const duplicateUri = fixtureStatus("different-local-id", {
      serverDomain: "relay.example",
      uri: genuinelyNew.uri,
      createdAt: genuinelyNew.createdAt,
      statusIdentity: {
        protocol: "activityPub",
        serverDomain: "relay.example",
        canonicalUri: genuinelyNew.uri,
        remoteId: "different-local-id",
      },
    });

    for (const status of [delayedOlder, genuinelyNew, duplicateUri]) {
      useAppStore.getState().applyStreamEvent({
        ...streamEvent(status),
        streamType: "public",
      });
    }
    flushTimelineStreamEventsForTest();

    const state = useAppStore.getState();
    expect(state.timelineDeferredKeys[federated.id]).toHaveLength(2);
    expect(state.timelineUnread[federated.id]).toBe(1);
  });

  it("coalesces a stream burst into one sequential refresh per visible custom/YQ column", async () => {
    const custom = {
      ...fixtureColumn("custom", "custom", 100),
      accountAcct: "legacy.bsky@bsky.social",
    };
    const yq = {
      ...fixtureColumn("yq", "yq", 100),
      accountAcct: "legacy.bsky@bsky.social",
      paneIndex: 1,
    };
    resetTimelineStore(
      [custom, yq],
      { custom: [status], yq: [status] },
      { activeTabs: { 0: custom.id, 1: yq.id } },
    );

    for (let index = 0; index < 80; index += 1) {
      useAppStore
        .getState()
        .applyStreamEvent(streamEvent(fixtureStatus(`burst-${index}`)));
    }
    flushTimelineStreamEventsForTest();
    expect(api.invokeReadCommand).not.toHaveBeenCalled();
    useAppStore.getState().applyTimelineCacheCommit();
    await flushAnalyticalTimelineRefreshesForTest();

    const refreshCalls = api.invokeReadCommand.mock.calls.filter(
      ([command]) => command === "refresh_timeline",
    );
    expect(refreshCalls).toHaveLength(2);
  });

  it("keeps a hidden analytical tab dirty instead of running its SQL", async () => {
    const visible = fixtureColumn("visible", "home", 100);
    const hidden = {
      ...fixtureColumn("hidden", "custom", 100),
      position: 1,
    };
    resetTimelineStore(
      [visible, hidden],
      { visible: [status], hidden: [status] },
      { activeTabs: { 0: visible.id } },
    );

    useAppStore
      .getState()
      .applyStreamEvent(streamEvent(fixtureStatus("hidden-dirty")));
    flushTimelineStreamEventsForTest();

    expect(api.invokeReadCommand).not.toHaveBeenCalled();
    expect(useAppStore.getState().timelineUnread[hidden.id]).toBe(1);
    useAppStore.getState().applyTimelineCacheCommit();
    useAppStore.getState().setActiveTab(0, hidden);
    await flushAnalyticalTimelineRefreshesForTest();
    expect(
      api.invokeReadCommand.mock.calls.filter(
        ([command]) => command === "refresh_timeline",
      ),
    ).toHaveLength(1);
  });

  it("keeps an active analytical timeline at a far anchor until it returns to the top", async () => {
    const custom = fixtureColumn("far-custom", "custom", 100);
    resetTimelineStore(
      [custom],
      { [custom.id]: [status] },
      {
        activeTabs: { 0: custom.id },
        timelineNearTop: { [custom.id]: false },
      },
    );

    useAppStore.getState().applyTimelineCacheCommit();
    await flushAnalyticalTimelineRefreshesForTest();

    expect(api.invokeReadCommand).not.toHaveBeenCalled();
    expect(useAppStore.getState().timelineUnread[custom.id] ?? 0).toBe(0);

    useAppStore.getState().trimTimelineToMaxStatuses(custom);
    await flushAnalyticalTimelineRefreshesForTest();
    expect(
      api.invokeReadCommand.mock.calls.filter(
        ([command]) => command === "refresh_timeline",
      ),
    ).toHaveLength(1);
  });

  it("reveals a deferred own post after smooth scrolling back to the top", async () => {
    const deferredHome = fixtureColumn("deferred-home", "home", 100);
    const older = fixtureStatus("older", {
      createdAt: new Date(1_000_000).toISOString(),
    });
    const posted = fixtureStatus("deferred-own-post", {
      createdAt: new Date(2_000_000).toISOString(),
    });
    resetTimelineStore(
      [deferredHome],
      { [deferredHome.id]: [older] },
      {
        timelineNearTop: { [deferredHome.id]: false },
      },
    );
    useAppStore.setState({ composeText: "hello", composeTarget: null });
    const queued = fixtureOutboxItem();
    api.invokeTypedCommand.mockResolvedValueOnce(queued);

    expect(await useAppStore.getState().post()).toBe(true);
    expect(
      useAppStore.getState().timelines[deferredHome.id]?.map(({ id }) => id),
    ).toEqual([older.id]);
    expect(useAppStore.getState().timelineUnread[deferredHome.id] ?? 0).toBe(0);
    expect(
      useAppStore.getState().timelineDeferredKeys[deferredHome.id],
    ).toBeUndefined();

    useAppStore.getState().applyComposeOutboxUpdate({
      item: { ...queued, state: "succeeded" },
      status: posted,
    });
    flushTimelineStreamEventsForTest();
    expect(useAppStore.getState().timelineUnread[deferredHome.id]).toBe(1);
    expect(
      useAppStore.getState().timelineDeferredKeys[deferredHome.id],
    ).toHaveLength(1);

    useAppStore.getState().trimTimelineToMaxStatuses(deferredHome);

    expect(
      useAppStore.getState().timelines[deferredHome.id]?.map(({ id }) => id),
    ).toEqual([posted.id, older.id]);
    expect(useAppStore.getState().timelineUnread[deferredHome.id] ?? 0).toBe(0);
    expect(
      useAppStore.getState().timelineDeferredKeys[deferredHome.id],
    ).toBeUndefined();
    expect(api.invokeReadCommand).not.toHaveBeenCalled();
  });

  it("drops a deferred search row when an update no longer matches", () => {
    const search = {
      ...fixtureColumn("deferred-search", "search", 100),
      columnParam: "needle",
    };
    const matching = fixtureStatus("deferred-search-hit", {
      content: "<p>needle</p>",
    });
    resetTimelineStore(
      [search],
      { [search.id]: [] },
      { timelineNearTop: { [search.id]: false } },
    );

    useAppStore.getState().applyStreamEvent(streamEvent(matching));
    flushTimelineStreamEventsForTest();
    expect(useAppStore.getState().timelineDeferredKeys[search.id]).toHaveLength(1);
    expect(useAppStore.getState().timelineUnread[search.id]).toBe(1);

    useAppStore.getState().applyStreamEvent({
      ...streamEvent({ ...matching, content: "<p>no match</p>" }),
      kind: "statusUpdate",
    });
    flushTimelineStreamEventsForTest();

    expect(useAppStore.getState().timelineDeferredKeys[search.id]).toBeUndefined();
    expect(useAppStore.getState().timelineUnread[search.id] ?? 0).toBe(0);
  });

  it("clears deferred unread state when the status is deleted", () => {
    const deferredHome = fixtureColumn("deleted-deferred-home", "home", 100);
    const pending = fixtureStatus("deleted-before-flush");
    resetTimelineStore(
      [deferredHome],
      { [deferredHome.id]: [] },
      { timelineNearTop: { [deferredHome.id]: false } },
    );

    useAppStore.getState().applyStreamEvent(streamEvent(pending));
    flushTimelineStreamEventsForTest();
    expect(
      useAppStore.getState().timelineDeferredKeys[deferredHome.id],
    ).toHaveLength(1);

    useAppStore.getState().applyStreamEvent({
      kind: "deleteStatus",
      streamType: "user",
      sourceAcct: pending.sourceAcct ?? "user@alpha.example",
      serverDomain: pending.serverDomain,
      statusId: pending.originalStatusId,
    });
    flushTimelineStreamEventsForTest();

    expect(
      useAppStore.getState().timelineDeferredKeys[deferredHome.id],
    ).toBeUndefined();
    expect(useAppStore.getState().timelineUnread[deferredHome.id] ?? 0).toBe(0);
  });

  it("evaluates search deltas locally without reloading the column", () => {
    const search = {
      ...fixtureColumn("search", "search", 100),
      columnParam: "needle",
      accountAcct: "legacy.bsky@bsky.social",
    };
    const matching = fixtureStatus("search-hit", {
      content: "<p>needle</p>",
      sourceAcct: "alice@mastodon.example",
    });
    resetTimelineStore([search], { search: [matching] });

    useAppStore.getState().applyStreamEvent({
      ...streamEvent({ ...matching, content: "<p>no longer matches</p>" }),
      kind: "statusUpdate",
    });
    flushTimelineStreamEventsForTest();
    expect(useAppStore.getState().timelines.search).toEqual([]);

    useAppStore
      .getState()
      .applyStreamEvent(streamEvent(fixtureStatus("new-search-hit", {
        content: "<p>new needle</p>",
      })));
    flushTimelineStreamEventsForTest();
    expect(useAppStore.getState().timelines.search).toHaveLength(1);
    expect(api.invokeReadCommand).not.toHaveBeenCalled();
  });

  it("reloads every unified Home column on an account stream resync", async () => {
    const mastodonHome = {
      ...fixtureColumn("mastodon-home", "home", 100),
      accountAcct: "alice@mastodon.example",
    };
    const blueskyHome = {
      ...fixtureColumn("bluesky-home", "home", 100),
      accountAcct: "alice.bsky@bsky.social",
    };
    const unrelatedList = {
      ...fixtureColumn("unrelated-list", "list", 100),
      accountAcct: "someone@else.example",
      columnParam: "17",
    };
    const profile = {
      ...fixtureColumn("open-profile", "profile", 80),
      dynamic: true,
      profile: {
        accountId: "profile-account",
        serverDomain: "alpha.example",
        sourceAcct: "user@alpha.example",
        acct: "profile@alpha.example",
        displayName: "Profile",
        avatar: "",
      },
    };
    resetTimelineStore(
      [mastodonHome, blueskyHome, unrelatedList],
      {},
      { dynamicColumns: [profile] },
    );

    useAppStore.getState().applyStreamEvent({
      kind: "resync",
      streamType: "resync",
      sourceAcct: "user@alpha.example",
      serverDomain: "alpha.example",
      generation: 2,
      sequence: 0,
    });

    await vi.waitFor(() => {
      expect(
        api.invokeReadCommand.mock.calls.filter(
          ([command]) => command === "refresh_timeline",
        ),
      ).toHaveLength(2);
    });
    expect(
      api.invokeReadCommand.mock.calls.some(
        ([command, args]) =>
          command === "refresh_timeline" &&
          args?.request?.columnType === "profile",
      ),
    ).toBe(false);
  });

  it("reloads the snapshot after a monotonic sequence gap", async () => {
    const sourceAcct = "user@gap.example";
    const serverDomain = "gap.example";
    const eventStatus = fixtureStatus("gap-status", {
      serverDomain,
      sourceAcct,
    });
    useAppStore.getState().applyStreamEvent({
      ...streamEvent(eventStatus),
      sourceAcct,
      serverDomain,
      generation: 1,
      sequence: 1,
    });
    useAppStore.getState().applyStreamEvent({
      ...streamEvent({ ...eventStatus, content: "<p>updated</p>" }),
      kind: "statusUpdate",
      sourceAcct,
      serverDomain,
      generation: 1,
      sequence: 3,
    });

    await vi.waitFor(() => {
      expect(
        api.invokeReadCommand.mock.calls.filter(
          ([command]) => command === "refresh_timeline",
        ),
      ).toHaveLength(1);
    });
    flushTimelineStreamEventsForTest();
  });

  it("routes a Mastodon home update to every Home column regardless of legacy account metadata", () => {
    const unified = fixtureColumn("unified-home", "home", 100);
    const mastodon = {
      ...fixtureColumn("mastodon-home", "home", 100),
      accountAcct: "alice@mastodon.example",
    };
    const bluesky = {
      ...fixtureColumn("bluesky-home", "home", 100),
      accountAcct: "alice.bsky@bsky.social",
    };
    resetTimelineStore([unified, mastodon, bluesky], {});
    const update = fixtureStatus("mastodon-update", {
      sourceAcct: "alice@mastodon.example",
      serverDomain: "mastodon.example",
      uri: "https://mastodon.example/@alice/1",
      statusIdentity: {
        protocol: "activityPub",
        serverDomain: "mastodon.example",
        canonicalUri: "https://mastodon.example/@alice/1",
        remoteId: "mastodon-update",
      },
    });

    useAppStore.getState().applyStreamEvent({
      ...streamEvent(update),
      sourceAcct: "alice@mastodon.example",
      serverDomain: "mastodon.example",
    });
    flushTimelineStreamEventsForTest();

    const timelines = useAppStore.getState().timelines;
    expect(timelines[unified.id]).toHaveLength(1);
    expect(timelines[mastodon.id]).toHaveLength(1);
    expect(timelines[bluesky.id]).toHaveLength(1);
  });

  it("routes a Bluesky revision delta to every unified Home column", () => {
    const unified = fixtureColumn("unified-bsky", "home", 100);
    const bluesky = {
      ...fixtureColumn("account-bsky", "home", 100),
      accountAcct: "@Alice.Bsky@BSKY.SOCIAL",
    };
    const mastodon = {
      ...fixtureColumn("account-mastodon", "home", 100),
      accountAcct: "alice@mastodon.example",
    };
    const uri = "at://did:plc:alice/app.bsky.feed.post/revision-1";
    const previous = fixtureStatus(uri, {
      sourceAcct: "alice.bsky@bsky.social",
      serverDomain: "bsky.social",
      uri,
      content: "<p>Before revision</p>",
      statusIdentity: {
        protocol: "atProto",
        serverDomain: "bsky.social",
        canonicalUri: uri,
        remoteId: uri,
      },
    });
    resetTimelineStore([unified, bluesky, mastodon], {
      [unified.id]: [previous],
      [bluesky.id]: [{ ...previous }],
    });
    const revision = { ...previous, content: "<p>Bluesky revision</p>" };

    useAppStore.getState().applyStreamEvent({
      kind: "newStatus",
      streamType: "user",
      sourceAcct: "alice.bsky@bsky.social",
      serverDomain: "bsky.social",
      status: revision,
    });
    flushTimelineStreamEventsForTest();

    const timelines = useAppStore.getState().timelines;
    expect(timelines[unified.id]).toHaveLength(1);
    expect(timelines[bluesky.id]).toHaveLength(1);
    expect(timelines[unified.id]?.[0].content).toBe("<p>Bluesky revision</p>");
    expect(timelines[bluesky.id]?.[0].content).toBe("<p>Bluesky revision</p>");
    expect(timelines[mastodon.id]).toHaveLength(1);
    expect(timelines[mastodon.id]?.[0].content).toBe("<p>Bluesky revision</p>");
  });

  it("routes an ActivityPub public event to every Public column", () => {
    const first = {
      ...fixtureColumn("public-first", "public", 100),
      accountAcct: "alice@mastodon.example",
    };
    const second = {
      ...fixtureColumn("public-second", "public", 100),
      accountAcct: "bob@pleroma.example",
    };
    resetTimelineStore([first, second], {});
    const update = fixtureStatus("public-update", {
      sourceAcct: "carol@akkoma.example",
      serverDomain: "akkoma.example",
    });

    useAppStore.getState().applyStreamEvent({
      ...streamEvent(update),
      streamType: "public",
    });
    flushTimelineStreamEventsForTest();

    expect(useAppStore.getState().timelines[first.id]).toHaveLength(1);
    expect(useAppStore.getState().timelines[second.id]).toHaveLength(1);
  });

  it("does not coalesce the same status across Unified Home and Public streams", () => {
    const home = fixtureColumn("home-cross-stream", "home", 100);
    const publicTimeline = fixtureColumn("public-cross-stream", "public", 100);
    resetTimelineStore([home, publicTimeline], {});
    const status = fixtureStatus("cross-stream-status", {
      sourceAcct: "alice@mastodon.example",
      serverDomain: "mastodon.example",
      uri: "https://mastodon.example/@alice/cross-stream-status",
      statusIdentity: {
        protocol: "activityPub",
        serverDomain: "mastodon.example",
        canonicalUri: "https://mastodon.example/@alice/cross-stream-status",
        remoteId: "cross-stream-status",
      },
    });

    useAppStore.getState().applyStreamEvent({
      ...streamEvent(status),
      streamType: "user",
      sourceAcct: "alice@mastodon.example",
    });
    useAppStore.getState().applyStreamEvent({
      ...streamEvent(status),
      streamType: "public",
      sourceAcct: "alice@mastodon.example",
    });
    flushTimelineStreamEventsForTest();

    expect(useAppStore.getState().timelines[home.id]).toHaveLength(1);
    expect(useAppStore.getState().timelines[publicTimeline.id]).toHaveLength(1);
  });

  it("routes an account notification to every Notification column", () => {
    const first = {
      ...fixtureColumn("notification-first", "notification", 100),
      accountAcct: "alice@mastodon.example",
    };
    const second = {
      ...fixtureColumn("notification-second", "notification", 100),
      accountAcct: "bob@pleroma.example",
    };
    resetTimelineStore([first, second], {});
    const notification = fixtureStatus("notification-status", {
      sourceAcct: "carol@akkoma.example",
      serverDomain: "akkoma.example",
      notificationId: "notification-1",
      notificationKind: "favourite",
    });

    useAppStore.getState().applyStreamEvent({
      ...streamEvent(notification),
      kind: "newNotification",
      streamType: "user:notification",
    });
    flushTimelineStreamEventsForTest();

    expect(useAppStore.getState().timelines[first.id]).toHaveLength(1);
    expect(useAppStore.getState().timelines[second.id]).toHaveLength(1);
  });

  it("keeps colliding notification ids from different signed-in accounts", () => {
    const unified = fixtureColumn("notification-collision", "notification", 100);
    resetTimelineStore([unified], {});
    const first = fixtureStatus("notification-status-first", {
      sourceAcct: "alice@example.social",
      serverDomain: "example.social",
      notificationId: "42",
      notificationKind: "favourite",
    });
    const second = fixtureStatus("notification-status-second", {
      sourceAcct: "bob@example.social",
      serverDomain: "example.social",
      notificationId: "42",
      notificationKind: "mention",
    });

    useAppStore.getState().applyStreamEvent({
      ...streamEvent(first),
      kind: "newNotification",
      streamType: "notification",
      sourceAcct: "alice@example.social",
    });
    useAppStore.getState().applyStreamEvent({
      ...streamEvent(second),
      kind: "newNotification",
      streamType: "notification",
      sourceAcct: "bob@example.social",
    });
    flushTimelineStreamEventsForTest();

    expect(useAppStore.getState().timelines[unified.id]).toHaveLength(2);
  });

  it("applies a matching list stream event directly without a DB round trip", () => {
    const list = {
      ...fixtureColumn("mastodon-list", "list", 100),
      accountAcct: "alice@mastodon.example",
      columnParam: "17",
    };
    resetTimelineStore([list], {});
    const update = fixtureStatus("list-update", {
      sourceAcct: "alice@mastodon.example",
      serverDomain: "mastodon.example",
      uri: "https://mastodon.example/@alice/2",
      statusIdentity: {
        protocol: "activityPub",
        serverDomain: "mastodon.example",
        canonicalUri: "https://mastodon.example/@alice/2",
        remoteId: "list-update",
      },
    });

    useAppStore.getState().applyStreamEvent({
      ...streamEvent(update),
      streamType: "list:17",
      sourceAcct: "alice@mastodon.example",
      serverDomain: "mastodon.example",
    });
    flushTimelineStreamEventsForTest();

    expect(useAppStore.getState().timelines[list.id]).toHaveLength(1);
    expect(api.invokeReadCommand).not.toHaveBeenCalled();
  });

  it("does not dispatch unchanged scroll state", () => {
    let notifications = 0;
    const unsubscribe = useAppStore.subscribe(() => {
      notifications += 1;
    });

    useAppStore.getState().setTimelineNearTop(home, true);
    expect(notifications).toBe(0);
    useAppStore.getState().setTimelineNearTop(home, false);
    expect(notifications).toBe(1);
    useAppStore.getState().setTimelineNearTop(home, false);
    expect(notifications).toBe(1);
    unsubscribe();
  });
});

function resetTimelineStore(
  columns: ColumnSummary[],
  timelines: Record<string, TimelineStatus[]>,
  extra: Partial<ReturnType<typeof useAppStore.getState>> = {},
) {
  let normalized = createTimelineEntityState();
  normalized = reduceTimelineEntities(
    normalized,
    columns.map((column) => ({
      type: "replaceColumn" as const,
      columnId: column.id,
      statuses: timelines[column.id] ?? [],
      limit: column.maxStatuses,
    })),
  );
  useAppStore.setState({
    snapshot: fixtureSnapshot(columns),
    entities: normalized.entities,
    timelineKeys: normalized.columnKeys,
    timelineDeferredKeys: normalized.deferredColumnKeys,
    canonicalIndex: normalized.canonicalIndex,
    timelines: normalized.timelines,
    dynamicColumns: [],
    loading: {},
    loadingMore: {},
    timelineHasMore: Object.fromEntries(
      columns.map((column) => [column.id, true]),
    ),
    timelineNearTop: {},
    activeTabs: {},
    timelineUnread: {},
    statusMutations: {},
    mutationStates: {},
    resourceStates: {},
    requestConfirmation: originalRequestConfirmation,
    streamPerformance: {
      batches: 0,
      lastBatchSize: 0,
      lastDurationMs: 0,
      p95DurationMs: 0,
    },
    mediaPreview: null,
    composeTarget: null,
    composeOutboxItems: [],
    composeOutboxOpen: false,
    error: undefined,
    ...extra,
  });
}

function fixtureColumn(
  id: string,
  columnType: string,
  maxStatuses: number,
): ColumnSummary {
  return {
    id,
    columnType,
    name: id,
    maxStatuses,
    paneIndex: 0,
    position: 0,
  };
}

function fixtureSnapshot(columns: ColumnSummary[]): AppSnapshot {
  return {
    version: "test",
    activeAcct: "user@alpha.example",
    accounts: [
      {
        acct: "user@alpha.example",
        serverDomain: "alpha.example",
        accountId: "account-alpha",
        displayName: "User",
        avatar: "",
        isActive: true,
        serverKind: "mastodon",
        characterLimit: 500,
        capabilities: {
          protocol: "activityPub",
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
      },
    ],
    columns,
    settings: {
      appearance: {
        avatar_shape: "Rounded",
        font_size: "Medium",
        cw_behavior: "Hide",
        nsfw_behavior: "Hide",
        theme: "Mocha",
        display_mode: "StarryEyes",
        visibility_background_enabled: false,
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

function fixtureStatus(
  id: string,
  overrides: Partial<TimelineStatus> = {},
): TimelineStatus {
  return {
    id,
    originalStatusId: id,
    sourceAcct: "user@alpha.example",
    accountId: "account-alpha",
    serverDomain: "alpha.example",
    uri: `https://alpha.example/statuses/${id}`,
    url: `https://alpha.example/statuses/${id}`,
    displayName: "User",
    acct: "user@alpha.example",
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
    ...overrides,
    statusIdentity: overrides.statusIdentity ?? {
      protocol: "activityPub",
      serverDomain: "alpha.example",
      canonicalUri: `https://alpha.example/statuses/${id}`,
      remoteId: id,
    },
  };
}

function streamEvent(status: TimelineStatus): TimelineStreamEvent {
  return {
    kind: "newStatus",
    streamType: "user",
    sourceAcct: status.sourceAcct ?? "user@alpha.example",
    serverDomain: status.serverDomain,
    status,
  };
}

function fixtureOutboxItem(
  overrides: Partial<ComposeOutboxItem> = {},
): ComposeOutboxItem {
  const timestamp = new Date(1_000_000).toISOString();
  return {
    id: "outbox-1",
    operationKind: "post",
    actingAccountAcct: "user@alpha.example",
    contentPreview: "hello",
    state: "queued",
    attempts: 0,
    nextAttemptAt: timestamp,
    createdAt: timestamp,
    updatedAt: timestamp,
    ...overrides,
  };
}
