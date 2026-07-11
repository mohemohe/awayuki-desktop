import React from "react";
import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { SettingsViewLayout } from "./SettingsViewLayout";

describe("Settings view fixture", () => {
  it("renders a selected panel and reports navigation through callbacks", () => {
    const onBack = vi.fn();
    const onSelectSection = vi.fn();
    const { container } = render(
      <SettingsViewLayout
        selectedSection="Appearance"
        onSelectSection={onSelectSection}
        onBack={onBack}
        saveIndicator={<span>Saved fixture</span>}
        panel={<div>Appearance fixture panel</div>}
      />,
    );

    expect(screen.getByText("Appearance fixture panel")).toBeVisible();
    expect(screen.getByText("Saved fixture")).toBeVisible();
    fireEvent.click(container.querySelector("#settings-tab-Account")!);
    expect(onSelectSection).toHaveBeenCalledWith("Account");
    fireEvent.click(screen.getByRole("button", { name: /Back|戻る/ }));
    expect(onBack).toHaveBeenCalledOnce();
  });
});
