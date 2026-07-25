import type { AppStartupProgressEvent } from "../../types/app";

export type BootState = {
  status: "idle" | "loading" | "ready" | "error" | "recovering";
  stage: "listeners" | "snapshot" | "timelines" | "complete";
  backendProgress?: AppStartupProgressEvent;
  error?: string;
};

export type BootAction =
  | { type: "begin"; recovering?: boolean }
  | { type: "listenerRegistrationFailed"; error: string }
  | { type: "backendProgress"; progress: AppStartupProgressEvent }
  | { type: "snapshotLoaded" }
  | { type: "ready" }
  | { type: "fail"; error: string };

export const initialBootState = (): BootState => ({
  status: "idle",
  stage: "listeners",
});

export function reduceBootState(
  state: BootState,
  action: BootAction,
): BootState {
  switch (action.type) {
    case "begin":
      return {
        status: action.recovering ? "recovering" : "loading",
        stage: "snapshot",
      };
    case "listenerRegistrationFailed":
      return {
        status: "error",
        stage: "listeners",
        error: action.error,
      };
    case "backendProgress":
      if (
        action.progress.status === "error" ||
        action.progress.stage === "error"
      ) {
        return {
          ...state,
          status: "error",
          backendProgress: action.progress,
          error: action.progress.message,
        };
      }
      return {
        ...state,
        status: state.status === "idle" ? "loading" : state.status,
        backendProgress: action.progress,
        error: undefined,
      };
    case "snapshotLoaded":
      return {
        status: state.status === "recovering" ? "recovering" : "loading",
        stage: "timelines",
      };
    case "ready":
      return { status: "ready", stage: "complete" };
    case "fail":
      return { status: "error", stage: state.stage, error: action.error };
  }
}
