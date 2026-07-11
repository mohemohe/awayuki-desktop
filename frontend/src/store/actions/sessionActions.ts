import type { StoreApi } from "zustand";
import type {
  AccountSummary,
  AppSnapshot,
  SettingsSnapshot,
  StatusBarSnapshot,
} from "../../types/app";
import { invokeCommand, invokeReadCommand } from "../../api/tauri";
import { t } from "../../i18n";
import { ConfirmationQueue } from "../../domain/confirmationQueue";
import { MutationLifecycle } from "../../domain/mutationLifecycle";
import { SettingsMutationCoordinator } from "../../domain/settingsMutations";
import { reconcileActiveTabs } from "../../utils/columns";
import { reduceBootState } from "../slices/session";
import type { AppStore } from "../appStore";

type TimelineInitialState = Pick<
  AppStore,
  "entities" | "timelineKeys" | "canonicalIndex" | "timelines"
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
  cancelAccountScopedFrontendWork: () => void;
  clearAccountScopedCaches: () => void;
  appStoreTimelineInitialState: () => TimelineInitialState;
  isUncertainMutationError: (error: unknown) => boolean;
};

export function createSessionActions({
  set,
  get,
  settingsCoordinator,
  mutationLifecycle,
  confirmationQueue,
  seedSettingsCoordinator,
  cancelAccountScopedFrontendWork,
  clearAccountScopedCaches,
  appStoreTimelineInitialState,
  isUncertainMutationError,
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
    const pendingBoot = reduceBootState(currentBoot, {
      type: "begin",
      recovering: currentBoot.status === "error",
    });
    set({
      boot: pendingBoot,
      error: undefined,
    });
    try {
      if (currentBoot.status === "error") {
        // The backend gate stays failed until this explicit mutation starts a
        // new, single initialization worker. Calling app_snapshot alone would
        // immediately return the previous failure forever.
        await invokeCommand("retry_runtime_initialization");
      } else {
        // React has mounted and App registered the startup progress listener
        // before this handshake. The backend must not start a long SQLite
        // migration during native setup, when no window can explain the wait.
        await invokeCommand("start_runtime_initialization");
      }
      const snapshot = await invokeReadCommand<AppSnapshot>("app_snapshot");
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
      const accounts =
        await invokeReadCommand<AccountSummary[]>("account_summaries");
      set((state) =>
        state.snapshot
          ? {
              snapshot: {
                ...state.snapshot,
                accounts,
              },
              error: undefined,
            }
          : {},
      );
    } catch (error) {
      set({ error: String(error) });
    }
  },
  loginWithInstanceDomain: async (domain) => {
    mutationLifecycle.invalidateAll(t("Account changed during an operation"));
    cancelAccountScopedFrontendWork();
    settingsCoordinator.resetScope();
    confirmationQueue.cancelAll();
    try {
      const snapshot = await invokeCommand<AppSnapshot>(
        "login_with_instance_domain",
        { request: { domain } },
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
      void get().loadStatusBar();
      await Promise.all(
        snapshot.columns.map((column) => get().loadTimeline(column, true)),
      );
      return true;
    } catch (error) {
      set({ error: String(error) });
      return false;
    }
  },
  loginWithBluesky: async (identifier, password) => {
    mutationLifecycle.invalidateAll(t("Account changed during an operation"));
    cancelAccountScopedFrontendWork();
    settingsCoordinator.resetScope();
    confirmationQueue.cancelAll();
    try {
      const snapshot = await invokeCommand<AppSnapshot>(
        "login_with_bluesky_app_password",
        { request: { identifier, password } },
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
      void get().loadStatusBar();
      await Promise.all(
        snapshot.columns.map((column) => get().loadTimeline(column, true)),
      );
      return true;
    } catch (error) {
      set({ error: String(error) });
      return false;
    }
  },
  loadStatusBar: async () => {
    try {
      const snapshot = await invokeReadCommand<
        Omit<StatusBarSnapshot, "fetchedAt">
      >("status_bar_snapshot");
      set({ statusBar: { ...snapshot, fetchedAt: Date.now() } });
    } catch (error) {
      set({ error: String(error) });
    }
  },
  switchAccount: async (acct) => {
    if (get().mutationStates["account:switch"]?.phase === "pending") return;
    mutationLifecycle.invalidateAll(t("Account changed during an operation"));
    confirmationQueue.cancelAll();
    try {
      const snapshot = await mutationLifecycle.run("account:switch", {
        execute: () =>
          invokeCommand<AppSnapshot>("switch_active_account", { acct }),
        isUncertain: isUncertainMutationError,
      });
      if (!snapshot) return;
      set((state) => ({
        // The active account is only the actor for mutations. Timeline data,
        // requests, unread counts, and pane selection are account-independent
        // and must survive an actor switch unchanged.
        snapshot: state.snapshot
          ? {
              ...state.snapshot,
              activeAcct: snapshot.activeAcct,
              accounts: snapshot.accounts,
            }
          : snapshot,
        error: undefined,
      }));
    } catch (error) {
      set({ error: String(error) });
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
    cancelAccountScopedFrontendWork();
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
        execute: () => invokeCommand<AppSnapshot>("logout_account", { acct }),
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
      if (snapshot.accounts.length > 0) {
        await Promise.all(
          snapshot.columns.map((column) => get().loadTimeline(column, true)),
        );
      }
    } catch (error) {
      set({ error: String(error) });
    }
  },
  };
}
