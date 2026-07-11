import React from "react";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { useAppStore } from "../../store/appStore";
import { LoginView } from "./LoginView";

const instanceLogin = vi.fn();
const blueskyLogin = vi.fn();

describe("LoginView submit intent", () => {
  beforeEach(() => {
    instanceLogin.mockReset().mockResolvedValue(false);
    blueskyLogin.mockReset().mockResolvedValue(false);
    useAppStore.setState({
      loginWithInstanceDomain: instanceLogin,
      loginWithBluesky: blueskyLogin,
      loginOpen: true,
    });
  });

  it("submits Bluesky credentials when Enter submits the password form", async () => {
    const user = userEvent.setup();
    render(<LoginView cancellable={false} />);

    await user.type(
      screen.getByLabelText("Username or email"),
      "alice.bsky.social",
    );
    const password = screen.getByLabelText("App password");
    await user.type(password, "abcd-efgh-ijkl-mnop{Enter}");

    await waitFor(() =>
      expect(blueskyLogin).toHaveBeenCalledWith(
        "alice.bsky.social",
        "abcd-efgh-ijkl-mnop",
      ),
    );
    expect(instanceLogin).not.toHaveBeenCalled();
  });

  it("uses the instance submit intent for its login button", async () => {
    render(<LoginView cancellable={false} />);
    const form = screen.getByRole("form", { name: "Instance login" });

    fireEvent.change(within(form).getByLabelText("Instance domain"), {
      target: { value: "example.social" },
    });
    fireEvent.click(within(form).getByRole("button", { name: "Log in" }));

    await waitFor(() =>
      expect(instanceLogin).toHaveBeenCalledWith("example.social"),
    );
    expect(blueskyLogin).not.toHaveBeenCalled();
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

    await waitFor(() =>
      expect(blueskyLogin).toHaveBeenCalledWith(
        "alice.bsky.social",
        "abcd-efgh-ijkl-mnop",
      ),
    );
    expect(instanceLogin).not.toHaveBeenCalled();
  });
});
