import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { AccountSummary } from "../../types/app";
import { useAppStore } from "../../store/appStore";
import { AccountQuickSwitcher } from "./AccountQuickSwitcher";

const accounts: AccountSummary[] = [
  {
    acct: "first@example.com",
    serverDomain: "example.com",
    accountId: "first",
    displayName: "First",
    avatar: "",
    isActive: true,
    serverKind: "mastodon",
    characterLimit: 500,
    rateLimit: null,
    capabilities: {} as AccountSummary["capabilities"],
  },
  {
    acct: "second@example.com",
    serverDomain: "example.com",
    accountId: "second",
    displayName: "Second",
    avatar: "",
    isActive: false,
    serverKind: "mastodon",
    characterLimit: 500,
    rateLimit: null,
    capabilities: {} as AccountSummary["capabilities"],
  },
];

describe("AccountQuickSwitcher", () => {
  beforeEach(() => {
    useAppStore.setState({
      switchAccount: vi.fn(() => new Promise<void>(() => undefined)),
    });
  });

  it("closes immediately without waiting for account reconciliation", async () => {
    const user = userEvent.setup();
    render(
      <AccountQuickSwitcher
        accounts={accounts}
        activeAcct="first@example.com"
      />,
    );

    await user.click(screen.getByTitle("Switch account"));
    await user.click(screen.getByRole("menuitemradio", { name: /Second/ }));

    expect(useAppStore.getState().switchAccount).toHaveBeenCalledWith(
      "second@example.com",
    );
    expect(screen.queryByRole("menu")).not.toBeInTheDocument();
  });
});
