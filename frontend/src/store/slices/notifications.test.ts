import { describe, expect, it } from "vitest";
import {
  clearUnreadResource,
  incrementUnreadResources,
} from "./notifications";

describe("notification slice reducers", () => {
  it("increments only addressed resources", () => {
    const initial = { a: 2, b: 7 };
    const next = incrementUnreadResources(initial, new Set(["a"]));
    expect(next).toEqual({ a: 3, b: 7 });
    expect(initial).toEqual({ a: 2, b: 7 });
  });

  it("clears one resource without clearing another", () => {
    expect(clearUnreadResource({ a: 2, b: 7 }, "a")).toEqual({ a: 0, b: 7 });
  });
});
