import React from "react";
import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { ComposeAreaView } from "./ComposeAreaView";

describe("ComposeArea view fixture", () => {
  it("renders controller slots and forwards enabled media drops", () => {
    const onDropFiles = vi.fn();
    const { container } = render(
      <ComposeAreaView
        height={240}
        mediaDropEnabled
        onDropFiles={onDropFiles}
      >
        <button type="button">Fixture account</button>
        <textarea aria-label="Fixture compose editor" />
      </ComposeAreaView>,
    );
    const file = new File(["fixture"], "fixture.png", { type: "image/png" });
    const section = container.querySelector("section")!;

    expect(screen.getByRole("button", { name: "Fixture account" })).toBeVisible();
    expect(section).toHaveStyle({ height: "240px" });
    fireEvent.drop(section, {
      dataTransfer: { files: [file], dropEffect: "copy" },
    });
    expect(onDropFiles).toHaveBeenCalledWith([file]);
  });

  it("keeps the controller responsible for validating editing-mode drops", () => {
    const onDropFiles = vi.fn();
    const { container } = render(
      <ComposeAreaView
        height={112}
        mediaDropEnabled={false}
        onDropFiles={onDropFiles}
      >
        <span>Editing fixture</span>
      </ComposeAreaView>,
    );

    fireEvent.drop(container.querySelector("section")!, {
      dataTransfer: {
        files: [new File(["fixture"], "fixture.png")],
        dropEffect: "none",
      },
    });
    expect(onDropFiles).toHaveBeenCalledOnce();
  });
});
