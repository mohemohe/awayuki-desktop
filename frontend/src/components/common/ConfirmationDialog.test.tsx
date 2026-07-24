import React from "react";
import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { useAppStore } from "../../store/appStore";
import { ConfirmationDialog } from "./ConfirmationDialog";

describe("ConfirmationDialog lifecycle", () => {
  it("resolves every dialog owned by the unmounted presenter without leaking promises", async () => {
    const first = useAppStore.getState().requestConfirmation({
      title: "first",
      message: "first message",
      confirmLabel: "confirm",
    });
    const second = useAppStore.getState().requestConfirmation({
      title: "second",
      message: "second message",
      confirmLabel: "confirm",
    });

    const view = render(<ConfirmationDialog />);
    expect(screen.getByRole("dialog")).toHaveTextContent("first");
    view.unmount();

    await expect(first).resolves.toBe(false);
    await expect(second).resolves.toBe(false);
    expect(useAppStore.getState().confirmationDialog).toBeUndefined();
  });
});
