import React from "react";
import { act, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { invokeTypedCommand, invokeTypedReadCommand } from "../../api/tauri";
import { setAppLocale } from "../../i18n";
import { useAppStore } from "../../store/appStore";
import type { WebSocketStatus } from "../../types/app";
import { WebSocketStatusControl } from "./WebSocketStatusControl";

vi.mock("../../api/tauri", () => ({ invokeTypedCommand: vi.fn(), invokeTypedReadCommand: vi.fn() }));
const connected: WebSocketStatus = { id: "home", account: "alice@example.com", server: "example.com", streamType: "Home", state: "connected", lastPingAt: "2026-09-06T01:02:03Z", lastPongAt: "2026-09-06T01:02:04Z", latencyMs: 42.12345 };

beforeEach(() => {
  vi.useFakeTimers();
  vi.clearAllMocks();
  setAppLocale("en");
  useAppStore.setState({ webSocketStatusOpen: false });
  vi.mocked(invokeTypedReadCommand).mockResolvedValue([connected, { ...connected, id: "list", streamType: "List", state: "reconnecting", lastPingAt: null, lastPongAt: null, latencyMs: null }]);
  vi.mocked(invokeTypedCommand).mockResolvedValue(undefined);
});
afterEach(() => vi.useRealTimers());

async function mount() {
  const view = render(<WebSocketStatusControl />);
  await act(async () => {});
  return view;
}

describe("WebSocket status", () => {
  it("shows an unknown count until the first status response", () => {
    vi.mocked(invokeTypedReadCommand).mockImplementationOnce(() => new Promise(() => {}));
    render(<WebSocketStatusControl />);
    expect(screen.getByRole("button", { name: "WebSocket status: —" })).toBeVisible();
  });

  it("counts only connected sockets and displays details and reconnect actions", async () => {
    await mount();
    fireEvent.click(screen.getByRole("button", { name: "WebSocket status: 1" }));
    const dialog = screen.getByRole("dialog", { name: "WebSocket status" });
    expect(within(dialog).getAllByText("alice@example.com")).toHaveLength(2);
    expect(within(dialog).getByText("Home")).toBeVisible();
    expect(within(dialog).getByText("List")).toBeVisible();
    expect(within(dialog).getByText("42.12 ms")).toBeVisible();
    expect(within(dialog).getAllByText("—")).toHaveLength(3);
    await act(async () => fireEvent.click(within(dialog).getByRole("button", { name: "Reconnect: alice@example.com Home" })));
    expect(invokeTypedCommand).toHaveBeenLastCalledWith("reconnect_web_socket", { id: "home" });
    await act(async () => fireEvent.click(within(dialog).getByRole("button", { name: "Reconnect all" })));
    expect(invokeTypedCommand).toHaveBeenLastCalledWith("reconnect_web_socket", { id: null });
    fireEvent.click(within(dialog).getAllByRole("button", { name: "Close" })[0]);
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(useAppStore.getState().webSocketStatusOpen).toBe(false);
  });

  it("does not overlap polls and ignores results from before a reconnect", async () => {
    await mount();
    let resolve!: (statuses: WebSocketStatus[]) => void;
    vi.mocked(invokeTypedReadCommand).mockImplementationOnce(() => new Promise((done) => { resolve = done; }));
    await act(async () => vi.advanceTimersByTime(4_000));
    expect(invokeTypedReadCommand).toHaveBeenCalledTimes(2);
    fireEvent.click(screen.getByRole("button", { name: "WebSocket status: 1" }));
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Reconnect all" })));
    await act(async () => resolve([]));
    expect(screen.getByRole("button", { name: "WebSocket status: 1" })).toBeVisible();
    await act(async () => vi.advanceTimersByTime(1_000));
    expect(invokeTypedReadCommand).toHaveBeenCalledTimes(3);
  });

  it("reports reconnect failure and resumes polling", async () => {
    await mount();
    vi.mocked(invokeTypedCommand).mockRejectedValueOnce(new Error("failure"));
    fireEvent.click(screen.getByRole("button", { name: "WebSocket status: 1" }));
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Reconnect all" })));
    expect(screen.getByRole("alert")).toHaveTextContent("Unable to reconnect WebSocket.");
    await act(async () => vi.advanceTimersByTime(1_000));
    expect(invokeTypedReadCommand).toHaveBeenCalledTimes(2);
  });
});
