import type { TimelineStatus } from "../../types/app";

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
    case "clearTarget":
      return {
        ...state,
        composeTarget: null,
        visibility:
          state.composeTarget?.kind === "reply"
            ? state.composeTarget.visibilityBeforeReply
            : state.visibility,
      };
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
