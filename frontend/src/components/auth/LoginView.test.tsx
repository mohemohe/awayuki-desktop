import React from "react";
import {
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  loginWithInstanceDomain: vi.fn(),
  loginWithBluesky: vi.fn(),
  setState: vi.fn(),
  invokeTypedCommand: vi.fn(),
}));

vi.mock("../../api/tauri", () => ({
  invokeTypedCommand: mocks.invokeTypedCommand,
}));
vi.mock("../../store/appStore", () => {
  const state = {
    loginWithInstanceDomain: mocks.loginWithInstanceDomain,
    loginWithBluesky: mocks.loginWithBluesky,
  };
  const useAppStore = Object.assign(
    (selector: (value: typeof state) => unknown) => selector(state),
    { setState: mocks.setState },
  );
  return { useAppStore };
});

import { LoginView } from "./LoginView";

describe("LoginView cancellation", () => {
  beforeEach(() => {
    mocks.loginWithInstanceDomain.mockReset();
    mocks.loginWithBluesky.mockReset();
    mocks.loginWithInstanceDomain.mockResolvedValue(false);
    mocks.loginWithBluesky.mockResolvedValue(false);
    mocks.setState.mockReset();
    mocks.invokeTypedCommand.mockReset();
  });

  it("submits Bluesky credentials when Enter submits the password form", async () => {
    const user = userEvent.setup();
    render(<LoginView cancellable={false} />);

    await user.type(
      screen.getByLabelText("Username or email"),
      "alice.bsky.social",
    );
    await user.type(
      screen.getByLabelText("App password"),
      "abcd-efgh-ijkl-mnop{Enter}",
    );

    await waitFor(() => expect(mocks.loginWithBluesky).toHaveBeenCalled());
    expect(mocks.loginWithBluesky.mock.calls[0]?.slice(0, 2)).toEqual([
      "alice.bsky.social",
      "abcd-efgh-ijkl-mnop",
    ]);
    expect(mocks.loginWithBluesky.mock.calls[0]?.[2]).toMatch(/^[0-9a-f-]{36}$/);
    expect(mocks.loginWithInstanceDomain).not.toHaveBeenCalled();
  });

  it("uses the instance submit intent for its login button", async () => {
    render(<LoginView cancellable={false} />);
    const form = screen.getByRole("form", { name: "Instance login" });
    fireEvent.change(within(form).getByLabelText("Instance domain"), {
      target: { value: "example.social" },
    });
    fireEvent.click(within(form).getByRole("button", { name: "Log in" }));

    await waitFor(() =>
      expect(mocks.loginWithInstanceDomain).toHaveBeenCalled(),
    );
    expect(mocks.loginWithInstanceDomain.mock.calls[0]?.[0]).toBe(
      "example.social",
    );
    expect(mocks.loginWithInstanceDomain.mock.calls[0]?.[1]).toMatch(
      /^[0-9a-f-]{36}$/,
    );
    expect(mocks.loginWithBluesky).not.toHaveBeenCalled();
  });

  it("uses the same Bluesky intent for its login button", async () => {
    const user = userEvent.setup();
    render(<LoginView cancellable={false} />);
    const form = screen.getByRole("form", { name: "Bluesky login" });
    await user.type(
      within(form).getByLabelText("Username or email"),
      "alice.bsky.social",
    );
    await user.type(
      within(form).getByLabelText("App password"),
      "abcd-efgh-ijkl-mnop",
    );
    await user.click(within(form).getByRole("button", { name: "Log in" }));

    await waitFor(() => expect(mocks.loginWithBluesky).toHaveBeenCalled());
    expect(mocks.loginWithBluesky.mock.calls[0]?.slice(0, 2)).toEqual([
      "alice.bsky.social",
      "abcd-efgh-ijkl-mnop",
    ]);
    expect(mocks.loginWithInstanceDomain).not.toHaveBeenCalled();
  });

  it("keeps Cancel available and cancels the active backend login operation", async () => {
    let resolveLogin: (value: boolean) => void = () => undefined;
    mocks.loginWithInstanceDomain.mockReturnValue(
      new Promise<boolean>((resolve) => {
        resolveLogin = resolve;
      }),
    );

    render(<LoginView cancellable />);
    fireEvent.click(
      within(screen.getByRole("form", { name: "Instance login" })).getByRole(
        "button",
        { name: "Log in" },
      ),
    );

    await waitFor(() =>
      expect(mocks.loginWithInstanceDomain).toHaveBeenCalledTimes(1),
    );
    const operationId = mocks.loginWithInstanceDomain.mock.calls[0][1] as string;
    expect(operationId).toMatch(/^[0-9a-f-]{36}$/);

    const cancel = screen.getByRole("button", { name: "Cancel" });
    expect(cancel).not.toBeDisabled();
    fireEvent.click(cancel);

    expect(mocks.invokeTypedCommand).toHaveBeenCalledWith("cancel_login_flow", {
      request: { targetOperationId: operationId },
    });
    expect(mocks.setState).toHaveBeenCalledWith({ loginOpen: false });
    resolveLogin(false);
  });
});
