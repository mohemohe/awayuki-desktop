import type { StoreApi } from "zustand";

import {
  invokeTypedCommand,
  invokeTypedReadCommand,
} from "../../api/tauri";
import type {
  ComposeOutboxItem,
  ComposeOutboxUpdatedEvent,
  TimelineStreamEvent,
} from "../../types/app";
import type { AppStore } from "../appStore";

type ComposeOutboxContext = {
  set: StoreApi<AppStore>["setState"];
  get: StoreApi<AppStore>["getState"];
};

const replaceItem = (
  items: ComposeOutboxItem[],
  incoming: ComposeOutboxItem,
) => {
  const existing = items.findIndex((item) => item.id === incoming.id);
  if (existing < 0) return [incoming, ...items];
  const next = items.slice();
  next[existing] = incoming;
  return next;
};

export function createComposeOutboxActions({
  set,
  get,
}: ComposeOutboxContext): Pick<
  AppStore,
  | "loadComposeOutbox"
  | "applyComposeOutboxUpdate"
  | "retryComposeOutboxItem"
  | "cancelComposeOutboxItem"
> {
  return {
    loadComposeOutbox: async () => {
      try {
        const items = await invokeTypedReadCommand("compose_outbox_items");
        set({ composeOutboxItems: Array.isArray(items) ? items : [] });
      } catch (error) {
        set({ error: String(error) });
      }
    },
    applyComposeOutboxUpdate: (event: ComposeOutboxUpdatedEvent) => {
      set((state) => ({
        composeOutboxItems: replaceItem(state.composeOutboxItems, event.item),
      }));
      if (!event.status) return;
      const timelineEvent: TimelineStreamEvent = {
        kind:
          event.item.operationKind === "edit" ? "statusUpdate" : "newStatus",
        streamType:
          event.item.operationKind === "edit" ? "status.update" : "user",
        sourceAcct: event.item.actingAccountAcct,
        serverDomain: event.status.serverDomain,
        status: event.status,
      };
      get().applyStreamEvent(timelineEvent);
    },
    retryComposeOutboxItem: async (id) => {
      try {
        const item = await invokeTypedCommand("retry_compose_outbox_item", {
          request: { id },
        });
        set((state) => ({
          composeOutboxItems: replaceItem(state.composeOutboxItems, item),
        }));
      } catch (error) {
        set({ error: String(error) });
      }
    },
    cancelComposeOutboxItem: async (id) => {
      try {
        const item = await invokeTypedCommand("cancel_compose_outbox_item", {
          request: { id },
        });
        set((state) => ({
          composeOutboxItems: replaceItem(state.composeOutboxItems, item),
        }));
      } catch (error) {
        set({ error: String(error) });
      }
    },
  };
}
