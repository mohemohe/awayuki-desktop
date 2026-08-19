import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { ColumnSummary, TimelineGap, TimelineStatus } from "../../types/app";
import { setAppLocale } from "../../i18n";

vi.mock("./TimelineStatusItem", () => ({
  StatusItem: ({ status }: { status: TimelineStatus }) => (
    <div data-testid={`status-${status.id}`}>{status.id}</div>
  ),
}));

import { TimelineStatusList } from "./TimelineStatusList";

describe("TimelineStatusList gaps", () => {
  it("renders a manual recovery button at the detected boundary", async () => {
    setAppLocale("en");
    const onLoadGap = vi.fn();
    const gap = {
      timelineType: "home",
      sourceAcct: "alice@example.test",
      boundaryStatusId: "boundary",
      boundaryServerDomain: "example.test",
      boundaryPosition: "2026-01-01T02:00:00.000Z",
      nextMaxStatusId: "boundary",
    } satisfies TimelineGap;
    const statuses = [
      { id: "boundary", createdAt: gap.boundaryPosition },
      { id: "older", createdAt: "2026-01-01T01:00:00.000Z" },
    ] as TimelineStatus[];

    render(
      <TimelineStatusList
        column={
          {
            id: "home",
            columnType: "home",
            name: "Home",
            maxStatuses: 100,
            paneIndex: 0,
            position: 0,
          } satisfies ColumnSummary
        }
        statuses={statuses}
        gaps={[gap]}
        virtualized={false}
        scrollTopRequest={0}
        isLoading={false}
        isLoadingMore={false}
        hasMore={false}
        hideLoadMore
        onLoadMore={() => undefined}
        onLoadGap={onLoadGap}
        onNearTopChange={() => undefined}
        onScrollTopComplete={() => undefined}
      />,
    );

    const button = screen.getByRole("button", { name: "Load missing posts" });
    expect(screen.getByTestId("status-boundary").compareDocumentPosition(button))
      .toBe(Node.DOCUMENT_POSITION_FOLLOWING);
    expect(button.compareDocumentPosition(screen.getByTestId("status-older")))
      .toBe(Node.DOCUMENT_POSITION_FOLLOWING);

    await userEvent.click(button);
    expect(onLoadGap).toHaveBeenCalledWith(gap);
  });
});
