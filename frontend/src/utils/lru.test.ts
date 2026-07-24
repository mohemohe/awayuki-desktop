import { describe, expect, it } from "vitest";
import { LruCache } from "./lru";

describe("LruCache", () => {
  it("evicts the least recently used value at capacity", () => {
    const cache = new LruCache<string, number>(2);
    cache.set("first", 1).set("second", 2);
    expect(cache.get("first")).toBe(1);

    cache.set("third", 3);

    expect(cache.get("second")).toBeUndefined();
    expect(cache.get("first")).toBe(1);
    expect(cache.get("third")).toBe(3);
    expect(cache.size).toBe(2);
  });

  it("distinguishes a cached undefined value from a miss", () => {
    const cache = new LruCache<string, undefined>(1);
    cache.set("value", undefined);
    expect(cache.has("value")).toBe(true);
    expect(cache.get("value")).toBeUndefined();
    expect(cache.has("value")).toBe(true);
  });

  it("expires entries deterministically", () => {
    let now = 100;
    const cache = new LruCache<string, number>(2, {
      ttlMs: 50,
      now: () => now,
    });
    cache.set("value", 1);
    now = 149;
    expect(cache.get("value")).toBe(1);
    now = 150;
    expect(cache.has("value")).toBe(false);
    expect(cache.size).toBe(0);
  });

  it("evicts by byte-style weight as well as item count", () => {
    const cache = new LruCache<string, string>(10, {
      maxWeight: 5,
      weight: (value) => value.length,
    });
    cache.set("first", "123").set("second", "456");

    expect(cache.has("first")).toBe(false);
    expect(cache.get("second")).toBe("456");
    expect(cache.weight).toBe(3);
  });
});
