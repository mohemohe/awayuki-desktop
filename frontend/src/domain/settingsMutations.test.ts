import { afterEach, describe, expect, it, vi } from "vitest";
import {
  SettingsMutationCoordinator,
  type SettingSaveState,
} from "./settingsMutations";

afterEach(() => vi.useRealTimers());

describe("SettingsMutationCoordinator", () => {
  it("debounces rapid input and persists only the newest draft", async () => {
    vi.useFakeTimers();
    const persisted: unknown[] = [];
    const coordinator = coordinatorFor(async (_key, value) => {
      persisted.push(value);
      return value;
    });
    coordinator.seed("preset_visibility", { entries: [] });

    const first = coordinator.enqueue("preset_visibility", "a");
    const second = coordinator.enqueue("preset_visibility", "away");
    const latest = coordinator.enqueue("preset_visibility", "awayuki");
    await vi.advanceTimersByTimeAsync(400);
    await Promise.all([first, second, latest]);

    expect(persisted).toEqual(["awayuki"]);
    expect(coordinator.state("preset_visibility")).toMatchObject({
      phase: "saved",
      draft: "awayuki",
      lastSaved: "awayuki",
    });
  });

  it("serializes writes and never commits an older response over a new draft", async () => {
    vi.useFakeTimers();
    const releases: Array<(value: string) => void> = [];
    const committed: unknown[] = [];
    const coordinator = coordinatorFor(
      (_key, value) =>
        new Promise<string>((resolve) => {
          releases.push(() => resolve(String(value)));
        }),
      (value) => committed.push(value),
    );

    const first = coordinator.enqueue("appearance", "old");
    await vi.advanceTimersByTimeAsync(400);
    const latest = coordinator.enqueue("appearance", "new");
    await vi.advanceTimersByTimeAsync(400);
    expect(releases).toHaveLength(1);
    expect(coordinator.state("appearance")?.draft).toBe("new");

    releases.shift()?.("old");
    await vi.waitFor(() => expect(releases).toHaveLength(1));
    expect(committed).toEqual([]);
    releases.shift()?.("new");
    await Promise.all([first, latest]);

    expect(committed).toEqual(["new"]);
    expect(coordinator.state("appearance")?.lastSaved).toBe("new");
  });

  it("ignores an in-flight response after an account scope change", async () => {
    vi.useFakeTimers();
    let release: ((value: string) => void) | undefined;
    const committed: unknown[] = [];
    const coordinator = coordinatorFor(
      () => new Promise<string>((resolve) => (release = resolve)),
      (value) => committed.push(value),
    );

    const save = coordinator.enqueue("debug", "account-a");
    await vi.advanceTimersByTimeAsync(400);
    coordinator.resetScope();
    release?.("account-a");
    await save;

    expect(committed).toEqual([]);
    expect(coordinator.state("debug")?.phase).toBe("conflict");
  });

  it("flushes a screen draft immediately when the settings view closes", async () => {
    vi.useFakeTimers();
    const persisted: unknown[] = [];
    const coordinator = coordinatorFor(async (_key, value) => {
      persisted.push(value);
      return value;
    });

    const save = coordinator.enqueue("confirmation", "draft-before-close");
    await coordinator.flush();
    await save;

    expect(persisted).toEqual(["draft-before-close"]);
    expect(coordinator.state("confirmation")?.phase).toBe("saved");
  });
});

function coordinatorFor(
  persist: (key: string, value: unknown) => Promise<unknown>,
  onPersisted: (value: unknown) => void = () => undefined,
) {
  const states: SettingSaveState[] = [];
  return new SettingsMutationCoordinator({
    persist,
    debounceMs: 400,
    onState: (state) => states.push(state),
    onPersisted: (_key, result) => onPersisted(result),
  });
}
