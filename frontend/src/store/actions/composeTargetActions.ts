import type { StoreApi } from "zustand";

import { htmlToPlainText } from "../../utils/format";
import { reduceComposeSlice, type ComposeVisibility } from "../slices/compose";
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
      const mention = `${status.acct.trim()} `;
      set((state) => {
        const current = state.composeText.trimEnd();
        return reduceComposeSlice(state, {
          type: "setTarget",
          target: { kind: "reply", status },
          text: current ? `${current}\n${mention}` : mention,
        });
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

function normalizeComposeVisibility(
  value: string | null | undefined,
): ComposeVisibility | null {
  if (!value) return null;
  const normalized = value.toLowerCase();
  return ["public", "unlisted", "private", "direct"].includes(normalized)
    ? (normalized as ComposeVisibility)
    : null;
}
