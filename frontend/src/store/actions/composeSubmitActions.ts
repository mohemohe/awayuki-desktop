import type { StoreApi } from "zustand";

import { invokeTypedCommandWithOperationId } from "../../api/tauri";
import type { MutationLifecycle } from "../../domain/mutationLifecycle";
import type { TimelineEntityOperation } from "../../domain/timelineEntities";
import type { AccountSummary, ColumnSummary, TimelineStatus } from "../../types/app";
import { matchPresetVisibility } from "../../utils/visibility";
import { reduceComposeSlice } from "../slices/compose";
import type { AppStore } from "../appStore";

type ComposeSubmitContext = {
  set: StoreApi<AppStore>["setState"];
  get: StoreApi<AppStore>["getState"];
  mutations: MutationLifecycle;
  requiredActingAccount: (state: AppStore) => AccountSummary;
  allColumns: (state: AppStore) => ColumnSummary[];
  statusMatchesDisplayFilter: (
    status: TimelineStatus,
    column: ColumnSummary,
  ) => boolean;
  timelineDisplayLimit: (column: ColumnSummary) => number;
  entityPatch: (
    state: AppStore,
    operations: TimelineEntityOperation[],
  ) => Partial<AppStore>;
  isUncertain: (error: unknown) => boolean;
};

export function createComposeSubmitActions({
  set,
  get,
  mutations,
  requiredActingAccount,
  allColumns,
  statusMatchesDisplayFilter,
  timelineDisplayLimit,
  entityPatch,
  isUncertain,
}: ComposeSubmitContext): Pick<AppStore, "post"> {
  return {
    post: async (options = {}) => {
      const { composeText, composeTarget, visibility, snapshot } = get();
      let actingAccount: AccountSummary;
      try {
        actingAccount = requiredActingAccount(get());
      } catch (error) {
        set({ error: String(error) });
        return false;
      }
      const hasMedia = Boolean(options.mediaIds?.length);
      const hasPoll = Boolean(options.poll?.options.length);
      const editing = composeTarget?.kind === "edit";
      if (editing && !composeText.trim()) return false;
      if (!editing && !composeText.trim() && !hasMedia && !hasPoll) return false;
      const resolvedVisibility =
        matchPresetVisibility(snapshot?.settings.presetVisibility, composeText) ??
        visibility;
      try {
        if (editing && composeTarget) {
          const updated = await get().editStatus(composeTarget.status, composeText, {
            visibility: resolvedVisibility,
            spoilerText: Object.prototype.hasOwnProperty.call(options, "spoilerText")
              ? (options.spoilerText ?? null)
              : composeTarget.status.spoilerText || null,
            sensitive: options.sensitive ?? composeTarget.status.sensitive,
          });
          if (!updated) return false;
          set((state) => reduceComposeSlice(state, { type: "clearDraft" }));
          return true;
        }
        const posted = await mutations.run("compose:submit", {
          execute: (operationId) =>
            invokeTypedCommandWithOperationId("post_status", {
              request: {
                actingAccountAcct: actingAccount.acct,
                status: composeText,
                visibility: resolvedVisibility,
                mediaIds: options.mediaIds,
                sensitive: options.sensitive ?? false,
                spoilerText: options.spoilerText,
                poll: options.poll,
                inReplyToId:
                  options.inReplyToId ??
                  (composeTarget?.kind === "reply"
                    ? composeTarget.status.originalStatusId
                    : undefined),
                quoteId:
                  options.quoteId ??
                  (composeTarget?.kind === "quote"
                    ? composeTarget.status.originalStatusId
                    : undefined),
              },
            }, operationId),
          isUncertain,
        });
        if (!posted) return false;
        set((state) => {
          const columns = allColumns(state).filter(
            (column) =>
              column.columnType === "home" &&
              statusMatchesDisplayFilter(posted, column),
          );
          const preserveAnchorColumns = new Set(
            columns
              .filter((column) => !(state.timelineNearTop[column.id] ?? true))
              .map((column) => column.id),
          );
          return {
            ...reduceComposeSlice(state, { type: "clearDraft" }),
            ...entityPatch(state, [
              {
                type: "upsertInColumns",
                columnIds: columns.map((column) => column.id),
                status: posted,
                limits: Object.fromEntries(
                  columns.map((column) => {
                    const configured = timelineDisplayLimit(column);
                    const currentLength = state.timelineKeys[column.id]?.length ?? 0;
                    return [
                      column.id,
                      preserveAnchorColumns.has(column.id)
                        ? configured
                        : currentLength > configured
                          ? undefined
                          : configured,
                    ];
                  }),
                ),
                preserveAnchorColumns,
              },
            ]),
          };
        });
        // The returned status is already inserted into every Unified Home
        // column above. SQLite persistence completes independently and its
        // timeline-cache-committed event invalidates analytical timelines.
        // Reloading here can race that commit and replace the posted status
        // with a stale cache snapshot.
        return true;
      } catch (error) {
        set({ error: String(error) });
        return false;
      }
    },
  };
}
