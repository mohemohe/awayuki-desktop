import type {
  ConfirmationDialogRequest,
  ConfirmationDialogState,
} from "../types/app";

type QueueEntry = {
  dialog: ConfirmationDialogState;
  resolve: (confirmed: boolean) => void;
};

/** Owns confirmation promises by dialog id and presents them FIFO. */
export class ConfirmationQueue {
  private entries: QueueEntry[] = [];

  constructor(
    private readonly onCurrentChange: (
      dialog: ConfirmationDialogState | undefined,
    ) => void,
  ) {}

  request(request: ConfirmationDialogRequest): Promise<boolean> {
    const id = crypto.randomUUID();
    return new Promise<boolean>((resolve) => {
      this.entries.push({ dialog: { ...request, id }, resolve });
      if (this.entries.length === 1) this.onCurrentChange(this.current());
    });
  }

  current() {
    return this.entries[0]?.dialog;
  }

  resolve(id: string, confirmed: boolean) {
    const index = this.entries.findIndex((entry) => entry.dialog.id === id);
    if (index < 0) return false;
    const [entry] = this.entries.splice(index, 1);
    entry.resolve(confirmed);
    if (index === 0) this.onCurrentChange(this.current());
    return true;
  }

  cancel(id: string) {
    return this.resolve(id, false);
  }

  cancelAll() {
    const entries = this.entries;
    this.entries = [];
    for (const entry of entries) entry.resolve(false);
    this.onCurrentChange(undefined);
  }

  size() {
    return this.entries.length;
  }
}
