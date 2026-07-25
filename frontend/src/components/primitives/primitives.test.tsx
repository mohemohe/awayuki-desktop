import React from "react";
import axe from "axe-core";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { Dialog } from "./Dialog";
import { MenuPopover } from "./Menu";
import { Tab, TabList } from "./Tabs";
import { Listbox, ListboxOption } from "./Listbox";
import { basicAccessibilityViolations } from "../../test/accessibility";

describe("accessible UI primitives", () => {
  it("has no automated axe violations in the shared primitive surface", async () => {
    const { container } = render(
      <>
        <Dialog open onClose={() => undefined} label="Preview">
          <button type="button">Close</button>
        </Dialog>
        <TabList label="Timelines">
          <Tab selected controls="panel-a" onSelect={() => undefined}>
            A
          </Tab>
          <Tab selected={false} controls="panel-b" onSelect={() => undefined}>
            B
          </Tab>
        </TabList>
        <section id="panel-a" role="tabpanel" aria-label="A timeline" />
        <section id="panel-b" role="tabpanel" aria-label="B timeline" hidden />
        <Listbox id="results" label="Results">
          <ListboxOption id="result-a" selected onMouseDown={() => undefined}>
            A
          </ListboxOption>
        </Listbox>
      </>,
    );

    const result = await axe.run(container, {
      // jsdom has no layout/color engine; contrast remains covered by the
      // packaged UI smoke test instead of producing false positives here.
      rules: { "color-contrast": { enabled: false } },
    });
    expect(result.violations).toEqual([]);
  });

  it("focuses a dialog, closes on Escape, and restores focus", async () => {
    const user = userEvent.setup();
    const close = vi.fn();
    const { rerender } = render(
      <>
        <button>Open</button>
        <Dialog open={false} onClose={close} label="Preview">
          <button>Close</button>
        </Dialog>
      </>,
    );
    await user.click(screen.getByRole("button", { name: "Open" }));
    rerender(
      <>
        <button>Open</button>
        <Dialog open onClose={close} label="Preview">
          <button>Close</button>
        </Dialog>
      </>,
    );
    await new Promise(requestAnimationFrame);
    expect(screen.getByRole("dialog", { name: "Preview" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Close" })).toHaveFocus();
    await user.keyboard("{Escape}");
    expect(close).toHaveBeenCalledOnce();
    rerender(
      <>
        <button>Open</button>
        <Dialog open={false} onClose={close} label="Preview">
          <button>Close</button>
        </Dialog>
      </>,
    );
    await new Promise(requestAnimationFrame);
    expect(screen.getByRole("button", { name: "Open" })).toHaveFocus();
  });

  it("navigates menu items with arrows", async () => {
    const user = userEvent.setup();
    render(
      <MenuPopover
        position={{ top: 0, left: 0 }}
        onClose={() => undefined}
        items={[
          { id: "one", label: "One", action: () => undefined },
          { id: "two", label: "Two", action: () => undefined },
        ]}
      />,
    );
    await new Promise(requestAnimationFrame);
    expect(screen.getByRole("menuitem", { name: "One" })).toHaveFocus();
    await user.keyboard("{ArrowDown}");
    expect(screen.getByRole("menuitem", { name: "Two" })).toHaveFocus();
  });

  it("exposes tabs and activates them with arrow keys", async () => {
    const user = userEvent.setup();
    const select = vi.fn();
    render(
      <TabList label="Timelines">
        <Tab selected controls="panel-a" onSelect={() => select("a")}>
          A
        </Tab>
        <Tab selected={false} controls="panel-b" onSelect={() => select("b")}>
          B
        </Tab>
      </TabList>,
    );
    const first = screen.getByRole("tab", { name: "A" });
    first.focus();
    await user.keyboard("{ArrowRight}");
    expect(screen.getByRole("tab", { name: "B" })).toHaveFocus();
    expect(select).toHaveBeenCalledWith("b");
    expect(basicAccessibilityViolations()).toEqual([]);
  });

  it("exposes listbox selection semantics", () => {
    render(
      <Listbox id="results" label="Results">
        <ListboxOption id="result-a" selected onMouseDown={() => undefined}>
          A
        </ListboxOption>
      </Listbox>,
    );
    expect(screen.getByRole("listbox", { name: "Results" })).toBeInTheDocument();
    expect(screen.getByRole("option", { name: "A" })).toHaveAttribute(
      "aria-selected",
      "true",
    );
    expect(basicAccessibilityViolations()).toEqual([]);
  });
});
