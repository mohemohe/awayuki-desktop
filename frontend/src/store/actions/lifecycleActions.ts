import type { ConfirmationQueue } from "../../domain/confirmationQueue";
import type { MutationLifecycle } from "../../domain/mutationLifecycle";
import type { AppStore } from "../appStore";

export function createLifecycleActions(
  confirmations: ConfirmationQueue,
  mutations: MutationLifecycle,
): Pick<
  AppStore,
  | "requestConfirmation"
  | "resolveConfirmation"
  | "cancelConfirmation"
  | "cancelAllConfirmations"
  | "runMutation"
> {
  return {
    requestConfirmation: (request) => confirmations.request(request),
    resolveConfirmation: (id, confirmed) => {
      confirmations.resolve(id, confirmed);
    },
    cancelConfirmation: (id) => {
      confirmations.cancel(id);
    },
    cancelAllConfirmations: () => {
      confirmations.cancelAll();
    },
    runMutation: (key, options) => mutations.run(key, options),
  };
}
