import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { EmotionalTextDropdown } from "./EmotionalTextDropdown";

describe("EmotionalTextDropdown", () => {
  it("shows every style preview and returns the selected style", () => {
    const onOpenChange = vi.fn();
    const onSelect = vi.fn();
    render(
      <EmotionalTextDropdown
        open
        onOpenChange={onOpenChange}
        onSelect={onSelect}
      />,
    );

    expect(screen.getAllByRole("menuitem")).toHaveLength(10);
    fireEvent.click(screen.getByRole("menuitem", { name: /Serif \(bold\)/ }));

    expect(onSelect).toHaveBeenCalledWith("boldSerif");
    expect(onOpenChange).toHaveBeenCalledWith(false);
  });
});
