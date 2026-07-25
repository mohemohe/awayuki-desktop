import React from "react";
import { render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { TimelineStatus } from "../../types/app";
import { NotificationMeta } from "./NotificationMeta";

function notificationStatus(
  overrides: Partial<TimelineStatus> = {},
): TimelineStatus {
  return {
    id: "notification-1",
    originalStatusId: "status-1",
    statusIdentity: {
      protocol: "atProto",
      serverDomain: "bsky.social",
      canonicalUri: "at://did:plc:self/app.bsky.feed.post/status-1",
      remoteId: "status-1",
    },
    sourceAcct: "me.bsky.social@bsky.social",
    accountId: "did:plc:self",
    serverDomain: "bsky.social",
    uri: "at://did:plc:self/app.bsky.feed.post/status-1",
    url: "https://bsky.app/profile/did:plc:self/post/status-1",
    displayName: "Me",
    acct: "@me.bsky.social",
    avatar: "",
    createdAt: "2026-07-17T22:44:26",
    originalCreatedAt: "2026-07-17T22:32:13",
    content: "<p>post</p>",
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
    notificationId: "notification-1",
    notificationKind: "favourite",
    notificationLabel: "Alice favourited",
    notificationAccountId: "did:plc:alice",
    notificationAcct: "@alice.bsky.social",
    notificationDisplayName: "Alice",
    notificationAvatar: "",
    notificationAccountEmojis: [],
    ...overrides,
  };
}

describe("NotificationMeta", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-07-17T23:00:00"));
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("shows the notification event time for non-reblog notifications", () => {
    render(
      <NotificationMeta
        status={notificationStatus()}
        onOpenUser={vi.fn()}
      />,
    );

    expect(screen.getByText("22:44:26")).toBeInTheDocument();
  });

  it("shows an event time even when the notification has no status", () => {
    render(
      <NotificationMeta
        status={notificationStatus({ originalCreatedAt: null })}
        onOpenUser={vi.fn()}
      />,
    );

    expect(screen.getByText("22:44:26")).toBeInTheDocument();
  });
});
