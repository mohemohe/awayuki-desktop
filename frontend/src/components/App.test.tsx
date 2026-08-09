import React from "react";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { AppSnapshot } from "../types/app";

const api = vi.hoisted(() => ({
  invokeCommand: vi.fn(),
  invokeReadCommand: vi.fn(),
  listen: vi.fn(),
  runtime: false,
  eventHandlers: new Map<
    string,
    (event: { payload: unknown }) => void
  >(),
}));

vi.mock("@tauri-apps/api/event", () => ({ listen: api.listen }));

vi.mock("../api/tauri", () => ({
  hasTauriRuntime: () => api.runtime,
  invokeCommand: api.invokeCommand,
  invokeTypedCommand: api.invokeCommand,
  invokeTypedCommandWithOperationId: api.invokeCommand,
  invokeReadCommand: api.invokeReadCommand,
  invokeTypedReadCommand: api.invokeReadCommand,
  invokeTypedReadCommandWithOperationId: api.invokeReadCommand,
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
    api.listen.mockReset();
    api.listen.mockImplementation(
      async (
        eventName: string,
        handler: (event: { payload: unknown }) => void,
      ) => {
        api.eventHandlers.set(eventName, handler);
        return () => {
          api.eventHandlers.delete(eventName);
        };
      },
    );
    api.runtime = false;
    api.eventHandlers.clear();
    useAppStore.setState({
      boot: { status: "idle", stage: "snapshot" },
      snapshot: undefined,
      error: undefined,
      timelines: {},
      dynamicColumns: [],
      activeTabs: {},
      composeOutboxItems: [],
      composeOutboxOpen: false,
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
      if (command === "compose_outbox_items") return [];
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

    const emitQueryMetrics = api.eventHandlers.get("timeline-query-metrics");
    await act(async () => {
      emitQueryMetrics?.({
        payload: {
          scannedCount: 10_001,
          matchedCount: 1,
          durationMs: 600,
          maxScannedRows: 25_000,
          maxDurationMs: 15_000,
          slow: true,
        },
      });
    });
    expect(useAppStore.getState().statusMessage).toContain("10001");
    expect(useAppStore.getState().statusMessage).toContain("600");
    expect(useAppStore.getState().statusMessage).toContain("YQ");

    await act(async () => {
      emitQueryMetrics?.({
        payload: {
          engine: "kq",
          scannedCount: 12_000,
          matchedCount: 2,
          durationMs: 700,
          maxScannedRows: 25_000,
          maxDurationMs: 15_000,
          slow: true,
        },
      });
    });
    expect(useAppStore.getState().statusMessage).toContain("KQ");
    expect(useAppStore.getState().statusMessage).not.toContain("YQ");

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
          stage: "sessions",
          status: "error",
          message: "The local database could not be initialized",
        },
      });
    });

    expect(screen.getByRole("alert")).toHaveTextContent(
      "Account sessions could not be restored",
    );
    expect(screen.getByRole("button", { name: "Retry" })).toBeEnabled();
  });

  it("stops before database initialization when a critical listener fails", async () => {
    api.runtime = true;
    api.listen.mockImplementation(async (eventName: string) => {
      if (eventName === "timeline-stream-event") {
        throw new Error("sensitive listener implementation detail");
      }
      return () => undefined;
    });

    render(<App />);

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "Registering application event listeners",
    );
    expect(api.invokeCommand).not.toHaveBeenCalledWith(
      "start_runtime_initialization",
    );
    expect(api.invokeReadCommand).not.toHaveBeenCalledWith("app_snapshot");
    expect(
      screen.queryByText("sensitive listener implementation detail"),
    ).not.toBeInTheDocument();

    api.listen.mockImplementation(async () => () => undefined);
    api.invokeReadCommand.mockImplementation(async (command: string) =>
      command === "status_bar_snapshot"
        ? { statusCount: 0, recentStatusCount: 0, uptimeSeconds: 0 }
        : emptySnapshot,
    );
    fireEvent.click(screen.getByRole("button", { name: "Retry" }));

    expect(await screen.findByRole("heading", { name: "awayuki" })).toBeVisible();
    expect(api.invokeCommand).toHaveBeenCalledTimes(1);
    expect(api.invokeCommand).toHaveBeenCalledWith(
      "start_runtime_initialization",
    );
  });
});
