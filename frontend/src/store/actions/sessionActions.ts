import type { StoreApi } from "zustand";
import type { SettingsSnapshot } from "../../types/app";
import {
  invokeTypedCommand,
  invokeTypedCommandWithOperationId,
  invokeTypedReadCommand,
} from "../../api/tauri";
import { IpcAppError } from "../../api/ipcErrors";
import { t } from "../../i18n";
import { ConfirmationQueue } from "../../domain/confirmationQueue";
import { MutationLifecycle } from "../../domain/mutationLifecycle";
import { SettingsMutationCoordinator } from "../../domain/settingsMutations";
import { reconcileActiveTabs } from "../../utils/columns";
import { reduceBootState } from "../slices/session";
import type { AppStore } from "../appStore";

type TimelineInitialState = Pick<
  AppStore,
  | "entities"
  | "timelineKeys"
  | "timelineDeferredKeys"
  | "canonicalIndex"
  | "timelines"
  | "timelineGaps"
  | "loadingTimelineGaps"
  | "timelineGapErrors"
>;

type SessionActionContext = {
  set: StoreApi<AppStore>["setState"];
  get: StoreApi<AppStore>["getState"];
  settingsCoordinator: SettingsMutationCoordinator<SettingsSnapshot>;
  mutationLifecycle: MutationLifecycle;
  confirmationQueue: ConfirmationQueue;
  seedSettingsCoordinator: (
    coordinator: SettingsMutationCoordinator<SettingsSnapshot>,
    settings: SettingsSnapshot,
  ) => void;
  cancelAccountScopedFrontendWork: () => () => void;
  cancelActingAccountMutations: () => () => void;
  clearAccountScopedCaches: () => void;
  appStoreTimelineInitialState: () => TimelineInitialState;
  isUncertainMutationError: (error: unknown) => boolean;
  reconcileViewerStates: (actingAccountAcct: string) => Promise<void>;
};

export function createSessionActions({
  set,
  get,
  settingsCoordinator,
  mutationLifecycle,
  confirmationQueue,
  seedSettingsCoordinator,
  cancelAccountScopedFrontendWork,
  cancelActingAccountMutations,
  clearAccountScopedCaches,
  appStoreTimelineInitialState,
  isUncertainMutationError,
  reconcileViewerStates,
}: SessionActionContext): Pick<
  AppStore,
  | "loadSnapshot"
  | "applyStartupProgress"
  | "refreshAccounts"
  | "loginWithInstanceDomain"
  | "loginWithBluesky"
  | "loadStatusBar"
  | "switchAccount"
  | "logoutAccount"
> {
  return {
  loadSnapshot: async () => {
    const currentBoot = get().boot;
    if (
      currentBoot.status === "loading" ||
      currentBoot.status === "recovering"
    ) {
      return;
    }
    const recoveringBackend =
      currentBoot.status === "error" && currentBoot.stage !== "listeners";
    const pendingBoot = reduceBootState(currentBoot, {
      type: "begin",
      recovering: recoveringBackend,
    });
    set({
      boot: pendingBoot,
      error: undefined,
    });
    try {
      if (recoveringBackend) {
        // The backend gate stays failed until this explicit mutation starts a
        // new, single initialization worker. Calling app_snapshot alone would
        // immediately return the previous failure forever.
        await invokeTypedCommand("retry_runtime_initialization");
      } else {
        // React has mounted and App registered the startup progress listener
        // before this handshake. The backend must not start a long SQLite
        // migration during native setup, when no window can explain the wait.
        await invokeTypedCommand("start_runtime_initialization");
      }
      const snapshot = await invokeTypedReadCommand("app_snapshot");
      seedSettingsCoordinator(settingsCoordinator, snapshot.settings);
      set((state) => ({
        boot: reduceBootState(state.boot, { type: "snapshotLoaded" }),
        snapshot,
        activeTabs: reconcileActiveTabs(
          [...snapshot.columns, ...state.dynamicColumns],
          state.activeTabs,
        ),
        error: undefined,
      }));
      void get().loadStatusBar();
      await Promise.all(
        snapshot.columns.map((column) => get().loadTimeline(column)),
      );
      set((state) => ({
        boot: reduceBootState(state.boot, { type: "ready" }),
      }));
    } catch (error) {
      set({
        boot: reduceBootState(get().boot, {
          type: "fail",
          error: String(error),
        }),
      });
    }
  },
  applyStartupProgress: (progress) => {
    set((state) => ({
      boot: reduceBootState(state.boot, {
        type: "backendProgress",
        progress,
      }),
    }));
  },
  refreshAccounts: async () => {
    try {
      const accounts = await invokeTypedReadCommand("account_summaries");
      set((state) =>
        state.snapshot
          ? {
              snapshot: {
                ...state.snapshot,
                accounts,
              },
            }
          : {},
      );
    } catch (error) {
      set({ error: String(error) });
    }
  },
  loginWithInstanceDomain: async (domain, requestedOperationId) => {
    mutationLifecycle.invalidateAll(t("Account changed during an operation"));
    const completeAccountTransition = cancelAccountScopedFrontendWork();
    settingsCoordinator.resetScope();
    confirmationQueue.cancelAll();
    try {
      const snapshot = await invokeTypedCommandWithOperationId(
        "login_with_instance_domain",
        { request: { domain } },
        requestedOperationId ?? crypto.randomUUID(),
      );
      clearAccountScopedCaches();
      seedSettingsCoordinator(settingsCoordinator, snapshot.settings);
      set((state) => ({
        ...appStoreTimelineInitialState(),
        snapshot,
        timelineUnread: {},
        statusMutations: {},
        resourceStates: {},
        loading: {},
        loadingMore: {},
        timelineHasMore: {},
        timelineNearTop: {},
        loginOpen: false,
        settingsOpen: false,
        activeTabs: reconcileActiveTabs(
          [...snapshot.columns, ...state.dynamicColumns],
          state.activeTabs,
        ),
        error: undefined,
      }));
      completeAccountTransition();
      void get().loadStatusBar();
      await Promise.all(
        snapshot.columns.map((column) => get().loadTimeline(column, true)),
      );
      return true;
    } catch (error) {
      if (!(error instanceof IpcAppError && error.code === "cancelled")) {
        set({ error: String(error) });
      }
      return false;
    } finally {
      completeAccountTransition();
    }
  },
  loginWithBluesky: async (identifier, password, requestedOperationId) => {
    mutationLifecycle.invalidateAll(t("Account changed during an operation"));
    const completeAccountTransition = cancelAccountScopedFrontendWork();
    settingsCoordinator.resetScope();
    confirmationQueue.cancelAll();
    try {
      const snapshot = await invokeTypedCommandWithOperationId(
        "login_with_bluesky_app_password",
        { request: { identifier, password } },
        requestedOperationId ?? crypto.randomUUID(),
      );
      clearAccountScopedCaches();
      seedSettingsCoordinator(settingsCoordinator, snapshot.settings);
      set((state) => ({
        ...appStoreTimelineInitialState(),
        snapshot,
        timelineUnread: {},
        statusMutations: {},
        resourceStates: {},
        loading: {},
        loadingMore: {},
        timelineHasMore: {},
        timelineNearTop: {},
        loginOpen: false,
        settingsOpen: false,
        activeTabs: reconcileActiveTabs(
          [...snapshot.columns, ...state.dynamicColumns],
          state.activeTabs,
        ),
        error: undefined,
      }));
      completeAccountTransition();
      void get().loadStatusBar();
      await Promise.all(
        snapshot.columns.map((column) => get().loadTimeline(column, true)),
      );
      return true;
    } catch (error) {
      if (!(error instanceof IpcAppError && error.code === "cancelled")) {
        set({ error: String(error) });
      }
      return false;
    } finally {
      completeAccountTransition();
    }
  },
  loadStatusBar: async () => {
    try {
      const snapshot = await invokeTypedReadCommand("status_bar_snapshot");
      set({ statusBar: { ...snapshot, fetchedAt: Date.now() } });
    } catch (error) {
      set({ error: String(error) });
    }
  },
  switchAccount: async (acct) => {
    if (get().mutationStates["account:switch"]?.phase === "pending") return;
    mutationLifecycle.invalidateAll(t("Account changed during an operation"));
    const completeAccountTransition = cancelActingAccountMutations();
    confirmationQueue.cancelAll();
    try {
      const result = await mutationLifecycle.run("account:switch", {
        execute: async (operationId) => {
          const snapshot = await invokeTypedCommandWithOperationId(
            "switch_active_account",
            { acct },
            operationId,
          );
          set((state) => ({
            // The backend switch is already complete. Publish the new actor
            // before refreshing viewer-specific flags so account controls do
            // not wait on a potentially expensive status reconciliation.
            snapshot: state.snapshot
              ? {
                  ...state.snapshot,
                  activeAcct: snapshot.activeAcct,
                  accounts: snapshot.accounts,
                }
              : snapshot,
          }));
          completeAccountTransition();
          let viewerStateError: unknown;
          if (snapshot.activeAcct) {
            try {
              await reconcileViewerStates(snapshot.activeAcct);
            } catch (error) {
              viewerStateError = error;
            }
          }
          return { snapshot, viewerStateError };
        },
        isUncertain: isUncertainMutationError,
      });
      if (!result) return;
      const { viewerStateError } = result;
      set({
        error: viewerStateError ? String(viewerStateError) : undefined,
      });
    } catch (error) {
      set({ error: String(error) });
    } finally {
      completeAccountTransition();
    }
  },
  logoutAccount: async (acct) => {
    const mutationKey = `account:logout:${acct}`;
    if (
      get().mutationStates[mutationKey]?.phase === "confirming" ||
      get().mutationStates[mutationKey]?.phase === "pending"
    ) {
      return;
    }
    mutationLifecycle.invalidateAll(t("Account changed during an operation"));
    const completeAccountTransition = cancelAccountScopedFrontendWork();
    settingsCoordinator.resetScope();
    confirmationQueue.cancelAll();
    try {
      const snapshot = await mutationLifecycle.run(mutationKey, {
        confirm: () =>
          confirmationQueue.request({
            title: t("Logout"),
            message: t("Log out {acct}?", { acct }),
            confirmLabel: t("Logout"),
            danger: true,
          }),
        execute: (operationId) =>
          invokeTypedCommandWithOperationId(
            "logout_account",
            { acct },
            operationId,
          ),
        isUncertain: isUncertainMutationError,
      });
      if (!snapshot) return;
      clearAccountScopedCaches();
      seedSettingsCoordinator(settingsCoordinator, snapshot.settings);
      set((state) => ({
        ...appStoreTimelineInitialState(),
        snapshot,
        timelineUnread: {},
        statusMutations: {},
        resourceStates: {},
        loading: {},
        loadingMore: {},
        timelineHasMore: {},
        timelineNearTop: {},
        activeTabs: reconcileActiveTabs(snapshot.columns, state.activeTabs),
        error: undefined,
      }));
      completeAccountTransition();
      if (snapshot.accounts.length > 0) {
        await Promise.all(
          snapshot.columns.map((column) => get().loadTimeline(column, true)),
        );
      }
    } catch (error) {
      set({ error: String(error) });
    } finally {
      completeAccountTransition();
    }
  },
  };
}
