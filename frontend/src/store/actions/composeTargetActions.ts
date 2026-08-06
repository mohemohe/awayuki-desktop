import type { StoreApi } from "zustand";

import { htmlToPlainText } from "../../utils/format";
import { reduceComposeSlice, type ComposeVisibility } from "../slices/compose";
import type { AccountSummary, TimelineStatus } from "../../types/app";
import type { AppStore } from "../appStore";

type ComposeTargetActionContext = {
  set: StoreApi<AppStore>["setState"];
  focusComposer: () => void;
};

export function createComposeTargetActions({
  set,
  focusComposer,
}: ComposeTargetActionContext): Pick<
  AppStore,
  "replyStatus" | "quoteStatus" | "beginEditStatus" | "clearComposeTarget"
> {
  const focus = () => requestAnimationFrame(focusComposer);
  return {
    replyStatus: (status) => {
      set((state) => {
        const selfReply = isSelfReplyTarget(status, findActingAccount(state));
        const current = state.composeText.trimEnd();
        const mention = `${status.acct.trim()} `;
        const visibilityBeforeReply =
          state.composeTarget?.kind === "reply"
            ? state.composeTarget.visibilityBeforeReply
            : state.visibility;
        return {
          ...reduceComposeSlice(state, {
            type: "setTarget",
            target: { kind: "reply", status, visibilityBeforeReply },
            ...(selfReply
              ? {}
              : { text: current ? `${current}\n${mention}` : mention }),
          }),
          visibility: normalizeComposeVisibility(status.visibility) ?? state.visibility,
        };
      });
      focus();
    },
    quoteStatus: (status) => {
      set((state) =>
        reduceComposeSlice(state, {
          type: "setTarget",
          target: { kind: "quote", status },
        }),
      );
      focus();
    },
    beginEditStatus: (status) => {
      set((state) => ({
        ...reduceComposeSlice(state, {
          type: "setTarget",
          target: { kind: "edit", status },
          text: htmlToPlainText(status.content),
        }),
        visibility: normalizeComposeVisibility(status.visibility) ?? "public",
      }));
      focus();
    },
    clearComposeTarget: () =>
      set((state) => reduceComposeSlice(state, { type: "clearTarget" })),
  };
}

function findActingAccount(state: AppStore): AccountSummary | undefined {
  const activeAcct = state.snapshot?.activeAcct?.trim();
  if (!activeAcct) return undefined;
  return state.snapshot?.accounts.find((account) => account.acct === activeAcct);
}

function isSelfReplyTarget(
  status: TimelineStatus,
  account: AccountSummary | undefined,
): boolean {
  if (!account) return false;
  if (
    status.accountId === account.accountId &&
    normalizeDomain(status.serverDomain) === normalizeDomain(account.serverDomain)
  ) {
    return true;
  }
  const statusAcct = normalizeAcct(status.acct);
  const accountAcct = normalizeAcct(account.acct);
  if (!statusAcct || !accountAcct) return false;
  if (statusAcct === accountAcct) return true;
  // ActivityPub servers report same-server authors with a domainless acct.
  return (
    !statusAcct.includes("@") &&
    `${statusAcct}@${normalizeDomain(status.serverDomain)}` === accountAcct
  );
}

function normalizeAcct(acct: string | null | undefined): string {
  return (acct ?? "").trim().replace(/^@+/, "").toLocaleLowerCase("en-US");
}

function normalizeDomain(domain: string | null | undefined): string {
  return (domain ?? "").trim().toLocaleLowerCase("en-US");
}

function normalizeComposeVisibility(
  value: string | null | undefined,
): ComposeVisibility | null {
  if (!value) return null;
  const normalized = value.toLowerCase();
  return ["public", "unlisted", "private", "direct"].includes(normalized)
    ? (normalized as ComposeVisibility)
    : null;
}
