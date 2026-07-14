import React from "react";
import { render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type {
  AccountProfileSummary,
  AppSnapshot,
  ColumnSummary,
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
    return (
      <div data-virtuoso-scroller="true">
        {Content ? <Content context={context} /> : null}
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

describe("UserProfilePane scroll ownership", () => {
  beforeEach(() => {
    mocks.invoke.mockReset();
    mocks.scrollTo.mockReset();
    mocks.scrollToIndex.mockReset();
    frontendRequestScheduler.resetForTest();
    mocks.invoke.mockImplementation(async (command: string) =>
      command === "account_profile" ? profile : [],
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
});
