import React from "react";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { AppSnapshot } from "../types/app";

const api = vi.hoisted(() => ({
  invokeCommand: vi.fn(),
  invokeReadCommand: vi.fn(),
  runtime: false,
  eventHandlers: new Map<
    string,
    (event: { payload: unknown }) => void
  >(),
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(
    async (
      eventName: string,
      handler: (event: { payload: unknown }) => void,
    ) => {
      api.eventHandlers.set(eventName, handler);
      return () => {
        api.eventHandlers.delete(eventName);
      };
    },
  ),
}));

vi.mock("../api/tauri", () => ({
  hasTauriRuntime: () => api.runtime,
  invokeCommand: api.invokeCommand,
  invokeReadCommand: api.invokeReadCommand,
}));

import { useAppStore } from "../store/appStore";
import { App } from "./App";

const emptySnapshot = {
  version: "test",
  accounts: [],
  activeAcct: null,
  columns: [],
  settings: { sidecars: { entries: [] } },
  database: {},
} as unknown as AppSnapshot;

describe("application boot recovery", () => {
  beforeEach(() => {
    api.invokeCommand.mockReset();
    api.invokeReadCommand.mockReset();
    api.runtime = false;
    api.eventHandlers.clear();
    useAppStore.setState({
      boot: { status: "idle", stage: "snapshot" },
      snapshot: undefined,
      error: undefined,
      timelines: {},
      dynamicColumns: [],
      activeTabs: {},
    });
  });

  it("shows a recoverable error instead of a permanent spinner", async () => {
    api.invokeReadCommand.mockRejectedValueOnce(
      new Error("sensitive database path should not be displayed"),
    );

    render(<App />);

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "Awayuki could not start",
    );
    expect(screen.getByRole("button", { name: "Retry" })).toBeEnabled();
    expect(
      screen.queryByText("sensitive database path should not be displayed"),
    ).not.toBeInTheDocument();
  });

  it("starts one backend retry before waiting for a new snapshot", async () => {
    let bootAttempts = 0;
    api.invokeReadCommand.mockImplementation(async (command: string) => {
      if (command === "status_bar_snapshot") {
        return { statusCount: 0, recentStatusCount: 0, uptimeSeconds: 0 };
      }
      bootAttempts += 1;
      if (bootAttempts === 1) throw new Error("temporary boot failure");
      return emptySnapshot;
    });

    render(<App />);
    fireEvent.click(
      await screen.findByRole("button", { name: "Retry" }),
    );

    expect(await screen.findByRole("heading", { name: "awayuki" })).toBeVisible();
    await waitFor(() =>
      expect(useAppStore.getState().boot.status).toBe("ready"),
    );
    expect(bootAttempts).toBe(2);
    expect(api.invokeCommand).toHaveBeenCalledTimes(2);
    expect(api.invokeCommand).toHaveBeenNthCalledWith(
      1,
      "start_runtime_initialization",
    );
    expect(api.invokeCommand).toHaveBeenCalledWith(
      "retry_runtime_initialization",
    );
    expect(api.invokeCommand.mock.invocationCallOrder[0]).toBeLessThan(
      api.invokeReadCommand.mock.invocationCallOrder[1] ?? Number.MAX_SAFE_INTEGER,
    );
  });

  it("subscribes before loading and renders backend startup progress", async () => {
    api.runtime = true;
    api.invokeReadCommand.mockImplementation(
      () => new Promise<never>(() => undefined),
    );

    render(<App />);

    await waitFor(() =>
      expect(api.eventHandlers.has("app-startup-progress")).toBe(true),
    );
    expect(api.invokeReadCommand).toHaveBeenCalledWith("app_snapshot");
    expect(api.invokeCommand).toHaveBeenCalledWith(
      "start_runtime_initialization",
    );

    const emitProgress = api.eventHandlers.get("app-startup-progress");
    await act(async () => {
      emitProgress?.({
        payload: {
          stage: "database",
          status: "running",
          message: "Preparing the portable database",
        },
      });
    });
    expect(screen.getByRole("status")).toHaveTextContent(
      "Updating the local database. Large caches can take several minutes.",
    );

    await act(async () => {
      emitProgress?.({
        payload: {
          stage: "sessions",
          status: "running",
          message: "Restoring session 1 of 2",
        },
      });
    });

    expect(screen.getByRole("status")).toHaveTextContent(
      "Restoring account sessions",
    );

    await act(async () => {
      emitProgress?.({
        payload: {
          stage: "error",
          status: "error",
          message: "The local database could not be initialized",
        },
      });
    });

    expect(screen.getByRole("alert")).toHaveTextContent(
      "Startup initialization failed",
    );
    expect(screen.getByRole("button", { name: "Retry" })).toBeEnabled();
  });
});
