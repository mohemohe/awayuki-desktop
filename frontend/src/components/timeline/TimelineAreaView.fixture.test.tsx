import React from "react";
import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import {
  EmptyTimelinePaneView,
  TimelineAreaView,
  TimelinePaneView,
} from "./TimelineAreaView";

describe("TimelineArea view fixtures", () => {
  it("renders controller-provided panes without reading application state", () => {
    render(
      <TimelineAreaView
        panes={[
          {
            paneIndex: 0,
            content: (
              <TimelinePaneView paneIndex={0} header="Home">
                <article>Fixture status</article>
              </TimelinePaneView>
            ),
          },
          {
            paneIndex: 1,
            content: (
              <EmptyTimelinePaneView
                title="Empty pane"
                message="No timeline tabs"
              />
            ),
          },
        ]}
      />,
    );

    expect(screen.getByText("Home")).toBeVisible();
    expect(screen.getByText("Fixture status")).toBeVisible();
    expect(screen.getByText("No timeline tabs")).toBeVisible();
    expect(document.querySelector('[data-pane-index="0"]')).not.toBeNull();
  });
});
