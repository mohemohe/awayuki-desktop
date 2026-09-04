import type { StoreApi } from "zustand";

import { invokeTypedCommandWithOperationId } from "../../api/tauri";
import type { MutationLifecycle } from "../../domain/mutationLifecycle";
import { t } from "../../i18n";
import type { AccountSummary } from "../../types/app";
import { matchPresetVisibility } from "../../utils/visibility";
import { reduceComposeSlice } from "../slices/compose";
import type { AppStore } from "../appStore";

type ComposeSubmitContext = {
  set: StoreApi<AppStore>["setState"];
  get: StoreApi<AppStore>["getState"];
  mutations: MutationLifecycle;
  requiredActingAccount: (state: AppStore) => AccountSummary;
  isUncertain: (error: unknown) => boolean;
  actingAccountScopeAvailable: () => boolean;
};

export function createComposeSubmitActions({
  set,
  get,
  mutations,
  requiredActingAccount,
  isUncertain,
  actingAccountScopeAvailable,
}: ComposeSubmitContext): Pick<AppStore, "post"> {
  return {
    post: async (options = {}) => {
      if (!actingAccountScopeAvailable()) return false;
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
        editing && composeTarget
          ? composeTarget.status.visibility
          : options.visibility ??
            matchPresetVisibility(
              snapshot?.settings.presetVisibility,
              composeText,
            ) ??
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
        const queued = await mutations.run("compose:submit", {
          execute: (operationId) =>
            invokeTypedCommandWithOperationId("enqueue_post_status", {
              request: {
                actingAccountAcct: actingAccount.acct,
                status: composeText,
                visibility: resolvedVisibility,
                mediaIds: options.mediaIds,
                sensitive: options.sensitive ?? false,
                spoilerText: options.spoilerText,
                poll: options.poll,
                inReplyToId: options.inReplyToId,
                inReplyToIdentity:
                  !options.inReplyToId && composeTarget?.kind === "reply"
                    ? composeTarget.status.statusIdentity
                    : undefined,
                quoteId: options.quoteId,
                quoteIdentity:
                  !options.quoteId && composeTarget?.kind === "quote"
                    ? composeTarget.status.statusIdentity
                    : undefined,
              },
            }, operationId),
          isUncertain,
        });
        if (!queued) return false;
        get().applyComposeOutboxUpdate({ item: queued });
        set((state) => ({
          ...reduceComposeSlice(state, { type: "clearDraft" }),
          statusMessage: t("Added to send queue"),
        }));
        return true;
      } catch (error) {
        set({ error: String(error) });
        return false;
      }
    },
  };
}
