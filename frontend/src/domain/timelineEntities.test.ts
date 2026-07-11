import { describe, expect, it } from "vitest";
import type { TimelineStatus } from "../types/app";
import {
  TIMELINE_HARD_MAX_STATUSES,
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

  it("hard-caps a multi-column ten-thousand-status fixture", () => {
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

    expect(state.entities.size).toBe(TIMELINE_HARD_MAX_STATUSES);
    for (const keys of Object.values(state.columnKeys)) {
      expect(keys).toHaveLength(TIMELINE_HARD_MAX_STATUSES);
    }
    expect(
      Object.values(state.columnKeys).reduce(
        (count, keys) => count + keys.length,
        0,
      ),
    ).toBe(12 * TIMELINE_HARD_MAX_STATUSES);

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
    expect(state.entities.size).toBeLessThanOrEqual(
      TIMELINE_HARD_MAX_STATUSES,
    );
    expect(
      Math.max(...Object.values(state.columnKeys).map((keys) => keys.length)),
    ).toBe(TIMELINE_HARD_MAX_STATUSES);
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
