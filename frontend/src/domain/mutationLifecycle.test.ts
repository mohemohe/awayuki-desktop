import { describe, expect, it, vi } from "vitest";
import { ConfirmationQueue } from "./confirmationQueue";
import { MutationLifecycle, type MutationState } from "./mutationLifecycle";

describe("ConfirmationQueue", () => {
  it("queues concurrent dialogs and resolves only the matching id", async () => {
    const visible: Array<string | undefined> = [];
    const queue = new ConfirmationQueue((dialog) => visible.push(dialog?.title));
    const first = queue.request(request("first"));
    const second = queue.request(request("second"));

    const firstId = queue.current()!.id;
    expect(queue.size()).toBe(2);
    queue.resolve(firstId, true);
    expect(await first).toBe(true);
    expect(queue.current()?.title).toBe("second");
    queue.cancel(queue.current()!.id);
    expect(await second).toBe(false);
    expect(visible).toEqual(["first", "second", undefined]);
  });

  it("cancels the owner dialog on unmount without touching another result", async () => {
    const queue = new ConfirmationQueue(() => undefined);
    const first = queue.request(request("first"));
    const second = queue.request(request("second"));
    queue.cancel(queue.current()!.id);
    queue.resolve(queue.current()!.id, true);

    await expect(first).resolves.toBe(false);
    await expect(second).resolves.toBe(true);
  });
});

describe("MutationLifecycle", () => {
  it("deduplicates double clicks and confirms once", async () => {
    const states: MutationState[] = [];
    const lifecycle = new MutationLifecycle((state) => states.push(state));
    let release: ((value: string) => void) | undefined;
    const execute = vi.fn(
      () => new Promise<string>((resolve) => (release = resolve)),
    );
    const confirm = vi.fn(async () => true);

    const first = lifecycle.run("database:clear", { confirm, execute });
    const duplicate = lifecycle.run("database:clear", { confirm, execute });
    await vi.waitFor(() => expect(execute).toHaveBeenCalledTimes(1));
    release?.("done");

    await expect(first).resolves.toBe("done");
    await expect(duplicate).resolves.toBe("done");
    expect(confirm).toHaveBeenCalledTimes(1);
    expect(states[states.length - 1]?.phase).toBe("succeeded");
  });

  it("marks response loss uncertain and never retries the mutation", async () => {
    const states: MutationState[] = [];
    const lifecycle = new MutationLifecycle((state) => states.push(state));
    const execute = vi.fn(async () => {
      throw new Error("IPC response lost after dispatch");
    });

    await lifecycle.run("post:1", {
      execute,
      isUncertain: (error) => String(error).includes("response lost"),
    });

    expect(execute).toHaveBeenCalledTimes(1);
    expect(states[states.length - 1]?.phase).toBe("uncertain");
  });

  it("does not commit a late mutation result after an account scope change", async () => {
    const states: MutationState[] = [];
    const lifecycle = new MutationLifecycle((state) => states.push(state));
    let release: ((value: string) => void) | undefined;
    const mutation = lifecycle.run("compose:submit", {
      execute: () =>
        new Promise<string>((resolve) => {
          release = resolve;
        }),
    });
    lifecycle.invalidateAll("account switched");
    release?.("old-account-result");

    await expect(mutation).resolves.toBeUndefined();
    expect(states[states.length - 1]).toMatchObject({
      phase: "uncertain",
    });
  });
});

function request(title: string) {
  return { title, message: title, confirmLabel: "ok" };
}
