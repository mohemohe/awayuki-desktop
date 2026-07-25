import type { StoreApi } from "zustand";

import {
  invokeTypedCommandWithOperationId,
  invokeTypedReadCommand,
} from "../../api/tauri";
import { canonicalStatusKey, type TimelineEntityOperation } from "../../domain/timelineEntities";
import { t } from "../../i18n";
import type {
  AccountSummary,
  DeleteStatusRequest,
  EditStatusRequest,
  PollSummary,
  TimelineStatus,
  VotePollRequest,
} from "../../types/app";
import { confirmStatusAction } from "../../utils/confirmation";
import type { AppStore } from "../appStore";

type StatusMutationContext = {
  set: StoreApi<AppStore>["setState"];
  get: StoreApi<AppStore>["getState"];
  requiredActingAccount: (state: AppStore) => AccountSummary;
  entityPatch: (
    state: AppStore,
    operations: TimelineEntityOperation[],
  ) => Partial<AppStore>;
  resolvedEntityFor: (state: AppStore, status: TimelineStatus) => TimelineStatus | undefined;
  syncResolvedConsumers: (
    state: AppStore,
    target: TimelineStatus,
    resolved: TimelineStatus,
  ) => Partial<AppStore>;
  sameCanonical: (left: TimelineStatus, right: TimelineStatus) => boolean;
  isUncertain: (error: unknown) => boolean;
  actingAccountScopeGeneration: () => number;
  actingAccountScopeAvailable: () => boolean;
};

function statusActionCapability(account: AccountSummary, action: string) {
  if (action.includes("favourite")) return account.capabilities.status.favourite;
  if (action.includes("reblog")) return account.capabilities.status.reblog;
  if (action.includes("bookmark")) return account.capabilities.status.bookmark;
  return false;
}

function optimisticStatusActionPatch(
  status: TimelineStatus,
  action: string,
): Partial<TimelineStatus> {
  switch (action) {
    case "favourite":
      return {
        favourited: true,
        favouritesCount: status.favourited
          ? status.favouritesCount
          : status.favouritesCount + 1,
      };
    case "unfavourite":
      return {
        favourited: false,
        favouritesCount: status.favourited
          ? Math.max(0, status.favouritesCount - 1)
          : status.favouritesCount,
      };
    case "reblog":
      return {
        reblogged: true,
        reblogsCount: status.reblogged
          ? status.reblogsCount
          : status.reblogsCount + 1,
      };
    case "unreblog":
      return {
        reblogged: false,
        reblogsCount: status.reblogged
          ? Math.max(0, status.reblogsCount - 1)
          : status.reblogsCount,
      };
    case "bookmark":
      return { bookmarked: true };
    case "unbookmark":
      return { bookmarked: false };
    default:
      return {};
  }
}

function desiredViewerFlag(action: string) {
  switch (action) {
    case "favourite":
      return ["favourited", true] as const;
    case "unfavourite":
      return ["favourited", false] as const;
    case "reblog":
      return ["reblogged", true] as const;
    case "unreblog":
      return ["reblogged", false] as const;
    case "bookmark":
      return ["bookmarked", true] as const;
    case "unbookmark":
      return ["bookmarked", false] as const;
    default:
      return undefined;
  }
}

export function createStatusMutationActions({
  set,
  get,
  requiredActingAccount,
  entityPatch,
  resolvedEntityFor,
  syncResolvedConsumers,
  sameCanonical,
  isUncertain,
  actingAccountScopeGeneration,
  actingAccountScopeAvailable,
}: StatusMutationContext): Pick<
  AppStore,
  "action" | "actionStatus" | "votePoll" | "editStatus" | "deleteStatus"
> {
  const beginMutation = (canonical: string, current: TimelineStatus) => {
    const operationId = crypto.randomUUID();
    set((state) => ({
      statusMutations: {
        ...state.statusMutations,
        [canonical]: { operationId, phase: "pending", beforeImage: current },
      },
    }));
    return operationId;
  };

  const actingAccountWithCapability = (
    capability: (account: AccountSummary) => boolean,
  ) => {
    const account = requiredActingAccount(get());
    if (!capability(account)) {
      throw new Error(t("This action is not supported by the selected account"));
    }
    return account;
  };

  return {
    action: async (_column, status, action) => {
      await get().actionStatus(status, action, true);
    },
    actionStatus: async (status, action, confirm = true) => {
      if (!actingAccountScopeAvailable()) return;
      const canonical = canonicalStatusKey(status);
      if (get().statusMutations[canonical]?.phase === "pending") return;
      let current = resolvedEntityFor(get(), status) ?? status;
      let actingAccount: AccountSummary;
      try {
        actingAccount = actingAccountWithCapability((account) =>
          statusActionCapability(account, action),
        );
      } catch (error) {
        set({ error: String(error) });
        return;
      }
      const accountScopeGeneration = actingAccountScopeGeneration();
      const operationId = beginMutation(canonical, current);
      const mutationIsCurrent = () =>
        actingAccountScopeGeneration() === accountScopeGeneration &&
        get().statusMutations[canonical]?.operationId === operationId;
      const abandonMutation = () => {
        set((state) => {
          if (state.statusMutations[canonical]?.operationId !== operationId) return {};
          const statusMutations = { ...state.statusMutations };
          delete statusMutations[canonical];
          return { statusMutations };
        });
      };
      try {
        const [viewerState] = await invokeTypedReadCommand(
          "status_viewer_states",
          {
            request: {
              actingAccountAcct: actingAccount.acct,
              identities: [current.statusIdentity],
            },
          },
        );
        if (!mutationIsCurrent()) {
          abandonMutation();
          return;
        }
        const displayedCurrent = current;
        current = {
          ...current,
          favourited: viewerState?.favourited ?? false,
          reblogged: viewerState?.reblogged ?? false,
          bookmarked: viewerState?.bookmarked ?? false,
        };
        const viewerPatch = {
          favourited: current.favourited,
          reblogged: current.reblogged,
          bookmarked: current.bookmarked,
        };
        set((state) => {
          const patch = entityPatch(state, [
            { type: "patchCanonical", target: displayedCurrent, patch: viewerPatch },
          ]);
          return {
            ...patch,
            statusMutations: {
              ...state.statusMutations,
              [canonical]: {
                operationId,
                phase: "pending",
                beforeImage: current,
              },
            },
            ...syncResolvedConsumers(state, displayedCurrent, current),
          };
        });

        const desired = desiredViewerFlag(action);
        if (desired && current[desired[0]] === desired[1]) {
          set((state) => ({
            statusMutations: {
              ...state.statusMutations,
              [canonical]: {
                operationId,
                phase: "confirmed",
                beforeImage: current,
              },
            },
          }));
          return;
        }
        if (confirm) {
          const confirmed = await confirmStatusAction(
            get().snapshot?.settings.confirmation,
            get().requestConfirmation,
            current,
            action,
          );
          if (!mutationIsCurrent()) {
            abandonMutation();
            return;
          }
          if (!confirmed) {
            set((state) => {
              if (state.statusMutations[canonical]?.operationId !== operationId) {
                return {};
              }
              const statusMutations = { ...state.statusMutations };
              delete statusMutations[canonical];
              return { statusMutations };
            });
            return;
          }
        }
        const optimisticPatch = optimisticStatusActionPatch(current, action);
        set((state) => {
          const patch = entityPatch(state, [
            { type: "patchCanonical", target: current, patch: optimisticPatch },
          ]);
          const optimistic = resolvedEntityFor({ ...state, ...patch }, current) ?? {
            ...current,
            ...optimisticPatch,
          };
          return {
            ...patch,
            statusMutations: {
              ...state.statusMutations,
              [canonical]: { operationId, phase: "pending", beforeImage: current },
            },
            ...syncResolvedConsumers(state, current, optimistic),
          };
        });
        const updated = await invokeTypedCommandWithOperationId("status_action", {
          request: {
            identity: current.statusIdentity,
            actingAccountAcct: actingAccount.acct,
            action,
          },
        }, operationId);
        if (!mutationIsCurrent()) {
          abandonMutation();
          return;
        }
        set((state) => {
          if (state.statusMutations[canonical]?.operationId !== operationId) return {};
          const patch = entityPatch(state, [
            { type: "replaceCanonical", target: current, status: updated },
          ]);
          const resolved = resolvedEntityFor({ ...state, ...patch }, current) ?? updated;
          return {
            ...patch,
            statusMutations: {
              ...state.statusMutations,
              [canonical]: { operationId, phase: "confirmed", beforeImage: current },
            },
            ...syncResolvedConsumers(state, current, resolved),
          };
        });
        get().applyTimelineCacheCommit();
      } catch (error) {
        if (!mutationIsCurrent()) {
          abandonMutation();
          return;
        }
        const uncertain = isUncertain(error);
        set((state) => {
          if (state.statusMutations[canonical]?.operationId !== operationId) {
            return { error: String(error) };
          }
          const patch = uncertain
            ? {}
            : entityPatch(state, [
                { type: "replaceCanonical", target: current, status: current },
              ]);
          return {
            ...patch,
            statusMutations: {
              ...state.statusMutations,
              [canonical]: {
                operationId,
                phase: uncertain ? "uncertain" : "failed",
                beforeImage: current,
                error: String(error),
              },
            },
            ...(!uncertain ? syncResolvedConsumers(state, current, current) : {}),
            error: uncertain
              ? `${t("The status action result is uncertain")}: ${String(error)}`
              : String(error),
          };
        });
      }
    },
    votePoll: async (status, choices): Promise<PollSummary | null> => {
      if (!status.poll) return null;
      const current = resolvedEntityFor(get(), status) ?? status;
      let actingAccount: AccountSummary;
      try {
        actingAccount = actingAccountWithCapability(
          (account) => account.capabilities.status.vote,
        );
      } catch (error) {
        set({ error: String(error) });
        return null;
      }
      const canonical = canonicalStatusKey(current);
      if (get().statusMutations[canonical]?.phase === "pending") return null;
      const operationId = beginMutation(canonical, current);
      try {
        const request: VotePollRequest = {
          identity: current.statusIdentity,
          actingAccountAcct: actingAccount.acct,
          pollId: current.poll?.id ?? status.poll.id,
          choices,
        };
        const poll = await invokeTypedCommandWithOperationId(
          "vote_poll",
          { request },
          operationId,
        );
        set((state) => {
          if (state.statusMutations[canonical]?.operationId !== operationId) return {};
          const patch = entityPatch(state, [
            { type: "patchCanonical", target: current, patch: { poll } },
          ]);
          const resolved = resolvedEntityFor({ ...state, ...patch }, current) ?? {
            ...current,
            poll,
          };
          return {
            ...patch,
            statusMutations: {
              ...state.statusMutations,
              [canonical]: { operationId, phase: "confirmed", beforeImage: current },
            },
            ...syncResolvedConsumers(state, current, resolved),
          };
        });
        get().applyTimelineCacheCommit();
        return poll;
      } catch (error) {
        set((state) => ({
          statusMutations: {
            ...state.statusMutations,
            [canonical]: {
              operationId,
              phase: isUncertain(error) ? "uncertain" : "failed",
              beforeImage: current,
              error: String(error),
            },
          },
          error: String(error),
        }));
        return null;
      }
    },
    editStatus: async (status, content, options = {}) => {
      const current = resolvedEntityFor(get(), status) ?? status;
      let actingAccount: AccountSummary;
      try {
        actingAccount = actingAccountWithCapability(
          (account) => account.capabilities.status.edit,
        );
      } catch (error) {
        set({ error: String(error) });
        return null;
      }
      const canonical = canonicalStatusKey(current);
      if (get().statusMutations[canonical]?.phase === "pending") return null;
      const operationId = beginMutation(canonical, current);
      try {
        const request: EditStatusRequest = {
          identity: current.statusIdentity,
          actingAccountAcct: actingAccount.acct,
          accountId: current.accountId,
          status: content,
          visibility: options.visibility ?? current.visibility,
          spoilerText: options.spoilerText ?? (current.spoilerText || null),
          sensitive: options.sensitive ?? current.sensitive,
        };
        await invokeTypedCommandWithOperationId(
          "enqueue_edit_status",
          { request },
          operationId,
        );
        set((state) => {
          if (state.statusMutations[canonical]?.operationId !== operationId) return {};
          return {
            statusMutations: {
              ...state.statusMutations,
              [canonical]: { operationId, phase: "confirmed", beforeImage: current },
            },
            statusMessage: t("Added to send queue"),
          };
        });
        return current;
      } catch (error) {
        set((state) => ({
          statusMutations: {
            ...state.statusMutations,
            [canonical]: {
              operationId,
              phase: isUncertain(error) ? "uncertain" : "failed",
              beforeImage: current,
              error: String(error),
            },
          },
          error: String(error),
        }));
        return null;
      }
    },
    deleteStatus: async (status) => {
      const current = resolvedEntityFor(get(), status) ?? status;
      let actingAccount: AccountSummary;
      try {
        actingAccount = actingAccountWithCapability(
          (account) => account.capabilities.status.delete,
        );
      } catch (error) {
        set({ error: String(error) });
        return false;
      }
      const canonical = canonicalStatusKey(current);
      if (get().statusMutations[canonical]?.phase === "pending") return false;
      const operationId = beginMutation(canonical, current);
      try {
        const request: DeleteStatusRequest = {
          identity: current.statusIdentity,
          actingAccountAcct: actingAccount.acct,
          accountId: current.accountId,
        };
        await invokeTypedCommandWithOperationId(
          "delete_own_status",
          { request },
          operationId,
        );
        set((state) => ({
          ...entityPatch(state, [{ type: "removeCanonical", target: current }]),
          statusMutations: {
            ...state.statusMutations,
            [canonical]: { operationId, phase: "confirmed", beforeImage: current },
          },
          mediaPreview:
            state.mediaPreview && sameCanonical(state.mediaPreview.status, current)
              ? null
              : state.mediaPreview,
          composeTarget:
            state.composeTarget && sameCanonical(state.composeTarget.status, current)
              ? null
              : state.composeTarget,
        }));
        get().applyTimelineCacheCommit();
        return true;
      } catch (error) {
        set((state) => ({
          statusMutations: {
            ...state.statusMutations,
            [canonical]: {
              operationId,
              phase: isUncertain(error) ? "uncertain" : "failed",
              beforeImage: current,
              error: String(error),
            },
          },
          error: String(error),
        }));
        return false;
      }
    },
  };
}
