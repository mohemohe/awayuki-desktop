import type {
  ConfirmationDialogState,
  MediaPreviewState,
} from "../../types/app";

export type OverlaySliceState = {
  mediaPreview?: MediaPreviewState | null;
  confirmationDialog?: ConfirmationDialogState;
};

export type OverlayAction =
  | { type: "openMedia"; preview: MediaPreviewState }
  | { type: "closeMedia" }
  | { type: "showConfirmation"; dialog?: ConfirmationDialogState }
  | { type: "reset" };

export function reduceOverlaySlice(
  state: OverlaySliceState,
  action: OverlayAction,
): OverlaySliceState {
  switch (action.type) {
    case "openMedia":
      return { ...state, mediaPreview: action.preview };
    case "closeMedia":
      return { ...state, mediaPreview: null };
    case "showConfirmation":
      return { ...state, confirmationDialog: action.dialog };
    case "reset":
      return { mediaPreview: null, confirmationDialog: undefined };
  }
}

