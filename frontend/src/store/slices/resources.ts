export type ResourcePhase =
  | "idle"
  | "loading"
  | "refreshing"
  | "succeeded"
  | "failed"
  | "cancelled"
  | "uncertain";

export type ResourceState = {
  generation: number;
  phase: ResourcePhase;
  error?: string;
};

export type ResourceStateMap = Record<string, ResourceState>;

export type ResourceAction =
  | { type: "begin"; key: string; generation: number; refreshing?: boolean }
  | { type: "succeed"; key: string; generation: number }
  | { type: "cancel"; key: string; generation: number }
  | { type: "fail"; key: string; generation: number; error: string }
  | { type: "uncertain"; key: string; generation: number; error: string }
  | { type: "clear"; key: string };

/** Resource-keyed errors prevent an unrelated success from clearing failures. */
export function reduceResourceStates(
  state: ResourceStateMap,
  action: ResourceAction,
): ResourceStateMap {
  if (action.type === "clear") {
    if (!(action.key in state)) return state;
    const next = { ...state };
    delete next[action.key];
    return next;
  }
  const current = state[action.key];
  if (current && current.generation > action.generation) return state;
  const phase: ResourcePhase =
    action.type === "begin"
      ? action.refreshing
        ? "refreshing"
        : "loading"
      : action.type === "succeed"
        ? "succeeded"
      : action.type === "cancel"
          ? "cancelled"
          : action.type === "fail"
            ? "failed"
            : action.type;
  return {
    ...state,
    [action.key]: {
      generation: action.generation,
      phase,
      ...(action.type === "fail" || action.type === "uncertain"
        ? { error: action.error }
        : {}),
    },
  };
}

export function resourceError(
  states: ResourceStateMap,
  key: string,
): string | undefined {
  return states[key]?.error;
}
