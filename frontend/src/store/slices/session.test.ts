import { describe, expect, it } from "vitest";
import { initialBootState, reduceBootState } from "./session";

describe("session boot reducer", () => {
  it("preserves the failed stage for recovery UI", () => {
    const loading = reduceBootState(initialBootState(), { type: "begin" });
    const timelines = reduceBootState(loading, { type: "snapshotLoaded" });
    expect(
      reduceBootState(timelines, { type: "fail", error: "offline" }),
    ).toEqual({ status: "error", stage: "timelines", error: "offline" });
  });

  it("marks a retry as recovery", () => {
    expect(
      reduceBootState(initialBootState(), { type: "begin", recovering: true }),
    ).toEqual({ status: "recovering", stage: "snapshot" });
  });

  it("tracks backend initialization without marking the app ready early", () => {
    const loading = reduceBootState(initialBootState(), { type: "begin" });

    expect(
      reduceBootState(loading, {
        type: "backendProgress",
        progress: {
          stage: "sessions",
          status: "running",
          message: "Restoring sessions",
        },
      }),
    ).toEqual({
      status: "loading",
      stage: "snapshot",
      backendProgress: {
        stage: "sessions",
        status: "running",
        message: "Restoring sessions",
      },
    });
  });

  it("turns a backend initialization error into a retryable boot state", () => {
    const loading = reduceBootState(initialBootState(), { type: "begin" });

    expect(
      reduceBootState(loading, {
        type: "backendProgress",
        progress: {
          stage: "error",
          status: "error",
          message: "Database initialization failed",
        },
      }),
    ).toEqual({
      status: "error",
      stage: "snapshot",
      backendProgress: {
        stage: "error",
        status: "error",
        message: "Database initialization failed",
      },
      error: "Database initialization failed",
    });
  });
});
