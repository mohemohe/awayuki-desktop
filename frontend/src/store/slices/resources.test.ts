import { describe, expect, it } from "vitest";
import { reduceResourceStates } from "./resources";

describe("resource slice reducer", () => {
  it("does not clear an unrelated resource error", () => {
    const failed = reduceResourceStates({}, {
      type: "fail",
      key: "timeline:a",
      generation: 1,
      error: "offline",
    });
    const next = reduceResourceStates(failed, {
      type: "succeed",
      key: "timeline:b",
      generation: 1,
    });

    expect(next["timeline:a"]?.error).toBe("offline");
    expect(next["timeline:b"]?.phase).toBe("succeeded");
  });

  it("ignores stale completions", () => {
    const current = reduceResourceStates({}, {
      type: "begin",
      key: "profile:1",
      generation: 2,
    });
    expect(
      reduceResourceStates(current, {
        type: "fail",
        key: "profile:1",
        generation: 1,
        error: "stale",
      }),
    ).toBe(current);
  });
});
