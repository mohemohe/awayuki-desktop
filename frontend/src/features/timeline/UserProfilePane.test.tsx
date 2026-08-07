import React from "react";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type {
  AccountProfileSummary,
  AppSnapshot,
  ColumnSummary,
  TimelineStatus,
} from "../../types/app";

const mocks = vi.hoisted(() => ({
  invoke: vi.fn(),
  scrollTo: vi.fn(),
  scrollToIndex: vi.fn(),
}));

vi.mock("react-virtuoso", async () => {
  const React = await import("react");
  type MockVirtuosoProps = {
    data?: unknown[];
    context?: unknown;
    components?: {
      Header?: React.ComponentType<{ context: unknown }>;
      EmptyPlaceholder?: React.ComponentType<{ context: unknown }>;
      Footer?: React.ComponentType<{ context: unknown }>;
    };
  };
  const Virtuoso = React.forwardRef<
    { scrollTo: typeof mocks.scrollTo; scrollToIndex: typeof mocks.scrollToIndex },
    MockVirtuosoProps
  >(function MockVirtuoso({ data = [], components, context }, ref) {
    React.useImperativeHandle(ref, () => ({
      scrollTo: mocks.scrollTo,
      scrollToIndex: mocks.scrollToIndex,
    }));
    const Content = data.length
      ? components?.Header
      : components?.EmptyPlaceholder;
    const Footer = data.length ? components?.Footer : undefined;
    return (
      <div data-virtuoso-scroller="true">
        {Content ? <Content context={context} /> : null}
        {Footer ? <Footer context={context} /> : null}
      </div>
    );
  });
  return { Virtuoso };
});

vi.mock("../../api/tauri", () => ({
  invokeTypedCommand: mocks.invoke,
  invokeTypedCommandWithOperationId: mocks.invoke,
  invokeTypedReadCommandWithOperationId: mocks.invoke,
}));

vi.mock("../../utils/renderMetrics", () => ({
  markNextRenderScenario: vi.fn(),
  measureNextPaint: vi.fn(),
}));

import { useAppStore } from "../../store/appStore";
import { frontendRequestScheduler } from "../../utils/requestScheduler";
import { UserProfilePane } from "./UserProfilePane";

const column: ColumnSummary = {
  id: "profile-column",
  columnType: "profile",
  name: "Profile",
  maxStatuses: 100,
  paneIndex: 0,
  position: 0,
  profile: {
    accountId: "profile-account",
    serverDomain: "example.test",
    sourceAcct: "viewer@example.test",
    acct: "profile@example.test",
    displayName: "Profile User",
    avatar: "",
  },
};

const profile: AccountProfileSummary = {
  id: "profile-account",
  serverDomain: "example.test",
  username: "profile",
  acct: "profile@example.test",
  url: "https://example.test/@profile",
  displayName: "Profile User",
  note: "Profile body",
  avatar: "",
  header: "",
  fields: [],
  accountEmojis: [],
  statusesCount: 10,
  followingCount: 20,
  followersCount: 30,
  isSelf: true,
  relationship: null,
  notificationMuted: false,
};

function status(id: string): TimelineStatus {
  return {
    id,
    originalStatusId: id,
    statusIdentity: {
      protocol: "activityPub",
      serverDomain: "example.test",
      canonicalUri: `https://example.test/statuses/${id}`,
      remoteId: id,
    },
    sourceAcct: "viewer@example.test",
    accountId: "profile-account",
    serverDomain: "example.test",
    uri: `https://example.test/statuses/${id}`,
    url: `https://example.test/statuses/${id}`,
    displayName: "Profile User",
    acct: "profile@example.test",
    avatar: "",
    createdAt: "2026-07-20T00:00:00.000Z",
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

describe("UserProfilePane scroll ownership", () => {
  beforeEach(() => {
    mocks.invoke.mockReset();
    mocks.scrollTo.mockReset();
    mocks.scrollToIndex.mockReset();
    frontendRequestScheduler.resetForTest();
    mocks.invoke.mockImplementation(async (command: string) =>
      command === "account_profile"
        ? profile
        : { statuses: [], hasMore: false, nextCursor: null },
    );
    useAppStore.setState({
      snapshot: {
        version: "test",
        accounts: [],
        activeAcct: null,
        columns: [column],
        settings: {
          appearance: { avatar_shape: "Rounded" },
        },
        database: {},
      } as unknown as AppSnapshot,
      entities: new Map(),
      timelineKeys: {},
      timelineDeferredKeys: {},
      canonicalIndex: new Map(),
      timelines: {},
      mutationStates: {},
      resourceStates: {},
      error: undefined,
    });
  });

  it("keeps profile chrome and statuses in one virtualized scroll surface", async () => {
    const { container, rerender } = render(
      <UserProfilePane column={column} scrollTopRequest={0} />,
    );

    expect(await screen.findByText("Profile User")).toBeInTheDocument();
    const scrollOwners = container.querySelectorAll(
      '[data-virtuoso-scroller="true"]',
    );
    expect(scrollOwners).toHaveLength(1);
    const scrollOwner = scrollOwners[0] as HTMLElement;
    expect(scrollOwner).toContainElement(container.querySelector(".user-name"));
    expect(scrollOwner).toContainElement(
      screen.getByRole("button", { name: "Posts" }),
    );
    expect(scrollOwner).toContainElement(
      screen.getByRole("button", { name: "Media" }),
    );

    rerender(<UserProfilePane column={column} scrollTopRequest={1} />);
    await waitFor(() =>
      expect(mocks.scrollTo).toHaveBeenCalledWith({
        top: 0,
        behavior: "smooth",
      }),
    );
    expect(mocks.scrollToIndex).not.toHaveBeenCalled();
  });

  it("loads the next profile page with the protocol cursor", async () => {
    mocks.invoke.mockImplementation(
      async (command: string, args?: Record<string, unknown>) => {
        if (command === "account_profile") return profile;
        if (command !== "account_timeline") return undefined;
        const request = args?.request as
          | { pinned?: boolean; onlyMedia?: boolean; cursor?: string }
          | undefined;
        if (request?.pinned || request?.onlyMedia) {
          return { statuses: [], hasMore: false, nextCursor: null };
        }
        return request?.cursor === "profile-cursor-1"
          ? {
              statuses: [status("older")],
              hasMore: false,
              nextCursor: null,
            }
          : {
              statuses: [status("newer")],
              hasMore: true,
              nextCursor: "profile-cursor-1",
            };
      },
    );

    render(<UserProfilePane column={column} scrollTopRequest={0} />);

    fireEvent.click(await screen.findByRole("button", { name: "Load More" }));

    await waitFor(() =>
      expect(mocks.invoke).toHaveBeenCalledWith(
        "account_timeline",
        expect.objectContaining({
          request: expect.objectContaining({
            cursor: "profile-cursor-1",
            offset: 1,
          }),
        }),
        expect.any(String),
      ),
    );
    expect(
      useAppStore.getState().timelines["profile:profile-column:posts"],
    ).toHaveLength(2);
  });

  it("does not restore a profile slice when load more resolves after unmount", async () => {
    let loadMoreOperationId: string | undefined;
    let resolveLoadMore!: (page: {
      statuses: TimelineStatus[];
      hasMore: boolean;
      nextCursor: null;
    }) => void;
    const loadMorePage = new Promise<{
      statuses: TimelineStatus[];
      hasMore: boolean;
      nextCursor: null;
    }>((resolve) => {
      resolveLoadMore = resolve;
    });
    mocks.invoke.mockImplementation(
      async (
        command: string,
        args?: Record<string, unknown>,
        operationId?: string,
      ) => {
        if (command === "account_profile") return profile;
        if (command !== "account_timeline") return undefined;
        const request = args?.request as
          | { pinned?: boolean; onlyMedia?: boolean; cursor?: string }
          | undefined;
        if (request?.pinned || request?.onlyMedia) {
          return { statuses: [], hasMore: false, nextCursor: null };
        }
        if (request?.cursor === "profile-cursor-1") {
          loadMoreOperationId = operationId;
          return loadMorePage;
        }
        return {
          statuses: [status("newer")],
          hasMore: true,
          nextCursor: "profile-cursor-1",
        };
      },
    );

    const { unmount } = render(
      <UserProfilePane column={column} scrollTopRequest={0} />,
    );
    fireEvent.click(await screen.findByRole("button", { name: "Load More" }));
    await waitFor(() =>
      expect(mocks.invoke).toHaveBeenCalledWith(
        "account_timeline",
        expect.objectContaining({
          request: expect.objectContaining({ cursor: "profile-cursor-1" }),
        }),
        expect.any(String),
      ),
    );

    unmount();
    expect(loadMoreOperationId).toEqual(expect.any(String));
    expect(
      useAppStore.getState().timelines["profile:profile-column:posts"],
    ).toBeUndefined();

    await act(async () => {
      resolveLoadMore({
        statuses: [status("older")],
        hasMore: false,
        nextCursor: null,
      });
      await loadMorePage;
    });

    expect(mocks.invoke).toHaveBeenCalledWith("cancel_timeline_query", {
      request: { targetOperationId: loadMoreOperationId },
    });
    expect(
      useAppStore.getState().timelines["profile:profile-column:posts"],
    ).toBeUndefined();
    expect(
      useAppStore.getState().timelineKeys["profile:profile-column:posts"],
    ).toBeUndefined();
  });
});
