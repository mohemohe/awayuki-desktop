import type { TimelineStatus } from "../../types/app";
import { statusEditText } from "../../utils/format";

export type ComposeVisibility = "public" | "unlisted" | "private" | "direct";

export type ComposeTarget =
  | {
      kind: "reply";
      status: TimelineStatus;
      visibilityBeforeReply: ComposeVisibility;
    }
  | {
      kind: "quote" | "edit";
      status: TimelineStatus;
    };

export type ComposeSliceState = {
  composeText: string;
  composeTarget?: ComposeTarget | null;
  visibility: ComposeVisibility;
};

export type ComposeAction =
  | { type: "setText"; text: string }
  | { type: "setTarget"; target: ComposeTarget; text?: string }
  | { type: "setVisibility"; visibility: ComposeVisibility }
  | { type: "clearTarget" }
  | { type: "clearDraft" }
  | { type: "reset" };

export const initialComposeSlice = (): ComposeSliceState => ({
  composeText: "",
  composeTarget: null,
  visibility: "public",
});

export function reduceComposeSlice(
  state: ComposeSliceState,
  action: ComposeAction,
): ComposeSliceState {
  switch (action.type) {
    case "setText":
      return { ...state, composeText: action.text };
    case "setTarget":
      return {
        ...state,
        composeTarget: action.target,
        ...(action.text === undefined ? {} : { composeText: action.text }),
      };
    case "setVisibility":
      return { ...state, visibility: action.visibility };
    case "clearTarget": {
      const isUntouchedReplyMention =
        state.composeTarget?.kind === "reply" &&
        state.composeText.trim() === state.composeTarget.status.acct.trim();
      const isUntouchedEdit =
        state.composeTarget?.kind === "edit" &&
        state.composeText === statusEditText(state.composeTarget.status);
      return {
        ...state,
        composeText:
          isUntouchedReplyMention || isUntouchedEdit ? "" : state.composeText,
        composeTarget: null,
        visibility:
          state.composeTarget?.kind === "reply"
            ? state.composeTarget.visibilityBeforeReply
            : state.visibility,
      };
    }
    case "clearDraft":
      return {
        ...state,
        composeText: "",
        composeTarget: null,
        visibility:
          state.composeTarget?.kind === "reply"
            ? state.composeTarget.visibilityBeforeReply
            : state.visibility,
      };
    case "reset":
      return initialComposeSlice();
  }
}
