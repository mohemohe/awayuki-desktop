import { describe, expect, it } from "vitest";
import type { TimelineStatus } from "../types/app";
import {
  canonicalStatusKey,
  createTimelineEntityState,
  reduceTimelineEntities,
  statusKey,
} from "./timelineEntities";

describe("timeline entity reducer", () => {
  it("shares one entity instance across columns", () => {
    const status = fixtureStatus("1", "alpha.example");
    const state = reduceTimelineEntities(createTimelineEntityState(), [
      {
        type: "replaceColumn",
        columnId: "home",
        statuses: [status],
        limit: 100,
      },
      {
        type: "replaceColumn",
        columnId: "local",
        statuses: [{ ...status }],
        limit: 100,
      },
    ]);

    expect(state.entities.size).toBe(1);
    expect(state.timelines.home[0]).toBe(state.timelines.local[0]);
    expect(state.columnKeys.home).toEqual([statusKey(status)]);
  });

  it("does not alias the same local ID from different servers", () => {
    const alpha = fixtureStatus("same-id", "alpha.example", { uri: "" });
    const beta = fixtureStatus("same-id", "beta.example", { uri: "" });
    let state = reduceTimelineEntities(createTimelineEntityState(), [
      {
        type: "replaceColumn",
        columnId: "multi-server",
        statuses: [alpha, beta],
        limit: 100,
      },
    ]);
    state = reduceTimelineEntities(state, [
      {
        type: "patchCanonical",
        target: alpha,
        patch: { favourited: true },
      },
    ]);

    expect(canonicalStatusKey(alpha)).not.toBe(canonicalStatusKey(beta));
    expect(state.timelines["multi-server"][0].favourited).toBe(true);
    expect(state.timelines["multi-server"][1].favourited).toBe(false);

    state = reduceTimelineEntities(state, [
      {
        type: "removeCanonicalId",
        serverDomain: alpha.serverDomain,
        statusId: alpha.id,
      },
    ]);
    expect(state.timelines["multi-server"]).toHaveLength(1);
    expect(state.timelines["multi-server"][0].serverDomain).toBe(
      beta.serverDomain,
    );
  });

  it("updates quote, reblog, and notification representations by canonical identity", () => {
    const subject = fixtureStatus("subject", "alpha.example");
    const quoteParent = fixtureStatus("parent", "alpha.example", {
      quote: { ...subject },
    });
    const reblog = fixtureStatus("boost-event", "alpha.example", {
      originalStatusId: subject.id,
      uri: subject.uri,
      content: subject.content,
    });
    const notification = fixtureStatus("notification-event", "alpha.example", {
      originalStatusId: subject.id,
      uri: subject.uri,
      content: subject.content,
      notificationId: "notification-1",
      notificationLabel: "favoured",
    });
    let state = reduceTimelineEntities(createTimelineEntityState(), [
      {
        type: "replaceColumn",
        columnId: "home",
        statuses: [subject, quoteParent, reblog],
        limit: 100,
      },
      {
        type: "replaceColumn",
        columnId: "notifications",
        statuses: [notification],
        limit: 100,
      },
    ]);
    const updated = { ...subject, content: "<p>edited</p>", favourited: true };
    state = reduceTimelineEntities(state, [
      { type: "replaceCanonical", target: subject, status: updated },
    ]);

    const [plain, parent, boosted] = state.timelines.home;
    const notificationResult = state.timelines.notifications[0];
    expect(plain.content).toBe("<p>edited</p>");
    expect(parent.quote).toBe(plain);
    expect(boosted.content).toBe("<p>edited</p>");
    expect(boosted.id).toBe("boost-event");
    expect(notificationResult.content).toBe("<p>edited</p>");
    expect(notificationResult.notificationId).toBe("notification-1");
  });

  it("keeps every notification event for the same canonical post", () => {
    const subject = fixtureStatus("subject", "alpha.example");
    const favourite = fixtureStatus("favourite-event", "alpha.example", {
      originalStatusId: subject.id,
      uri: subject.uri,
      notificationId: "notification-favourite",
      notificationKind: "favourite",
      notificationLabel: "Alice favourited",
      sourceAcct: "viewer@alpha.example",
      createdAt: "2026-07-13T13:45:43.212Z",
    });
    const reblog = fixtureStatus("reblog-event", "alpha.example", {
      originalStatusId: subject.id,
      uri: subject.uri,
      notificationId: "notification-reblog",
      notificationKind: "reblog",
      notificationLabel: "Bob boosted",
      sourceAcct: "viewer@alpha.example",
      createdAt: "2026-07-13T13:45:44.377Z",
    });
    let state = reduceTimelineEntities(createTimelineEntityState(), [
      {
        type: "replaceColumn",
        columnId: "notifications",
        statuses: [favourite],
        limit: 100,
      },
    ]);

    state = reduceTimelineEntities(state, [
      {
        type: "upsertInColumns",
        columnIds: ["notifications"],
        status: reblog,
        limits: { notifications: 100 },
      },
    ]);

    expect(state.timelines.notifications).toHaveLength(2);
    expect(
      state.timelines.notifications.map((status) => status.notificationId),
    ).toEqual(["notification-reblog", "notification-favourite"]);
    expect(
      state.timelines.notifications.map((status) => status.notificationKind),
    ).toEqual(["reblog", "favourite"]);
  });

  it("separates colliding notification ids from different source accounts", () => {
    const notification = fixtureStatus("event", "alpha.example", {
      notificationId: "42",
      sourceAcct: "alice@alpha.example",
    });
    const otherViewer = {
      ...notification,
      sourceAcct: "bob@alpha.example",
    };

    expect(statusKey(notification)).not.toBe(statusKey(otherViewer));
  });

  it("batches inserts without changing stable ordering or existing positions", () => {
    const existing = fixtureStatus("existing", "alpha.example", {
      createdAt: new Date(100_000).toISOString(),
    });
    const older = fixtureStatus("older", "alpha.example", {
      createdAt: new Date(50_000).toISOString(),
    });
    let state = reduceTimelineEntities(createTimelineEntityState(), [
      {
        type: "replaceColumn",
        columnId: "home",
        statuses: [existing, older],
        limit: 100,
      },
    ]);
    const sameTimeFirst = fixtureStatus("same-time-first", "alpha.example", {
      createdAt: new Date(75_000).toISOString(),
    });
    const sameTimeSecond = fixtureStatus("same-time-second", "alpha.example", {
      createdAt: new Date(75_000).toISOString(),
    });
    state = reduceTimelineEntities(state, [
      {
        type: "upsertInColumns",
        columnIds: ["home"],
        status: sameTimeFirst,
        limits: { home: 100 },
      },
      {
        type: "upsertInColumns",
        columnIds: ["home"],
        status: sameTimeSecond,
        limits: { home: 100 },
      },
      {
        type: "upsertInColumns",
        columnIds: ["home"],
        status: {
          ...existing,
          createdAt: new Date(25_000).toISOString(),
          content: "<p>updated</p>",
        },
        limits: { home: 100 },
      },
    ]);

    expect(state.timelines.home.map(({ id }) => id)).toEqual([
      "existing",
      "same-time-first",
      "same-time-second",
      "older",
    ]);
    expect(state.timelines.home[0]?.content).toBe("<p>updated</p>");
  });

  it("retains the requested multi-column ten-thousand-status fixture", () => {
    const statuses = Array.from({ length: 10_000 }, (_, index) =>
      fixtureStatus(String(index), "alpha.example", {
        createdAt: new Date(20_000_000 - index * 1_000).toISOString(),
      }),
    );
    const operations = Array.from({ length: 12 }, (_, index) => ({
      type: "replaceColumn" as const,
      columnId: `column-${index}`,
      statuses,
      limit: 10_000,
    }));
    let state = reduceTimelineEntities(
      createTimelineEntityState(),
      operations,
    );

    expect(state.entities.size).toBe(10_000);
    for (const keys of Object.values(state.columnKeys)) {
      expect(keys).toHaveLength(10_000);
    }
    expect(
      Object.values(state.columnKeys).reduce(
        (count, keys) => count + keys.length,
        0,
      ),
    ).toBe(12 * 10_000);

    const olderPage = Array.from({ length: 250 }, (_, index) =>
      fixtureStatus(`older-${index}`, "alpha.example", {
        createdAt: new Date(9_000_000 - index * 1_000).toISOString(),
      }),
    );
    state = reduceTimelineEntities(state, [
      {
        type: "appendPage",
        columnId: "column-0",
        statuses: olderPage,
      },
    ]);
    expect(state.columnKeys["column-0"]).toHaveLength(10_250);
    expect(state.entities.size).toBe(10_250);

    const columnIds = operations.map((operation) => operation.columnId);
    state = reduceTimelineEntities(
      state,
      Array.from({ length: 500 }, (_, index) => ({
        type: "upsertInColumns" as const,
        columnIds,
        status: fixtureStatus(`burst-${index}`, "alpha.example", {
          createdAt: new Date(30_000_000 + index * 1_000).toISOString(),
        }),
        limits: Object.fromEntries(
          columnIds.map((columnId) => [columnId, 10_000]),
        ),
      })),
    );
    expect(state.entities.size).toBe(10_000);
    expect(
      Math.max(...Object.values(state.columnKeys).map((keys) => keys.length)),
    ).toBe(10_000);
  });

  it("preserves ordered keys when a far-from-top column suppresses a prepend", () => {
    const statuses = Array.from({ length: 5 }, (_, index) =>
      fixtureStatus(String(index), "alpha.example", {
        createdAt: new Date(10_000 - index * 1_000).toISOString(),
      }),
    );
    let state = reduceTimelineEntities(createTimelineEntityState(), [
      {
        type: "replaceColumn",
        columnId: "home",
        statuses,
        limit: 5,
      },
    ]);
    const anchorKeys = state.columnKeys.home;
    state = reduceTimelineEntities(state, [
      {
        type: "upsertInColumns",
        columnIds: ["home"],
        status: fixtureStatus("new", "alpha.example", {
          createdAt: new Date(11_000).toISOString(),
        }),
        limits: { home: 5 },
        preserveAnchorColumns: new Set(["home"]),
      },
    ]);

    expect(state.columnKeys.home).toEqual(anchorKeys);
    expect(state.timelines.home[state.timelines.home.length - 1]?.id).toBe(
      statuses[statuses.length - 1]?.id,
    );
    expect(state.deferredColumnKeys.home).toHaveLength(1);

    state = reduceTimelineEntities(state, [
      { type: "flushDeferredColumn", columnId: "home", limit: 5 },
    ]);

    expect(state.timelines.home[0]?.id).toBe("new");
    expect(state.deferredColumnKeys.home).toBeUndefined();
  });
});

export function fixtureStatus(
  id: string,
  serverDomain: string,
  overrides: Partial<TimelineStatus> = {},
): TimelineStatus {
  const numericId = Number(id);
  return {
    id,
    originalStatusId: id,
    sourceAcct: `user@${serverDomain}`,
    accountId: `account-${serverDomain}`,
    serverDomain,
    uri: `https://${serverDomain}/statuses/${id}`,
    url: `https://${serverDomain}/statuses/${id}`,
    displayName: "User",
    acct: `user@${serverDomain}`,
    avatar: "",
    createdAt: new Date(
      1_000_000 - (Number.isFinite(numericId) ? numericId : 0) * 1_000,
    ).toISOString(),
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
      serverDomain,
      canonicalUri: `https://${serverDomain}/statuses/${id}`,
      remoteId: id,
    },
  };
}
