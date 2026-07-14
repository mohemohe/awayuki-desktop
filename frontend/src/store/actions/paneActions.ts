import type { ColumnSummary, TimelineStatus, UserProfileTarget } from "../../types/app";
import { createColumn } from "../../utils/columns";
import { t } from "../../i18n";
import type { DynamicPaneDescriptor } from "../slices/panes";

type OpenDynamicPane = (
  descriptor: DynamicPaneDescriptor,
  options?: { load?: boolean },
) => ColumnSummary;

export type PaneActions = {
  addBookmarksPane: () => void;
  addFavouritesPane: () => void;
  openUserBookmarksPane: (target: UserProfileTarget) => void;
  openSearchPane: (query: string) => void;
  openThreadPane: (status: TimelineStatus) => void;
  openAirContextPane: (status: TimelineStatus) => void;
  openUserPane: (status: TimelineStatus) => void;
};

/** UI intent -> dynamic pane descriptor mapping, independent of Zustand/Tauri. */
export function createPaneActions(open: OpenDynamicPane): PaneActions {
  return {
    addBookmarksPane: () => {
      open({ resourceKey: "bookmarks:", column: createColumn(0, 0, "bookmarks") });
    },
    addFavouritesPane: () => {
      open({ resourceKey: "favourites:", column: createColumn(0, 0, "favourites") });
    },
    openUserBookmarksPane: (target) => {
      if (!target.accountId || !target.serverDomain) return;
      const columnParam = JSON.stringify({
        accountId: target.accountId,
        serverDomain: target.serverDomain,
      });
      const acct = target.acct || target.accountId;
      open({
        resourceKey: `user_bookmarks:${columnParam}`,
        column: {
          ...createColumn(0, 0, "user_bookmarks"),
          columnParam,
          name: t("Bookmarks by {acct}", { acct: `@${acct.replace(/^@/, "")}` }),
          maxStatuses: 100,
        },
      });
    },
    openSearchPane: (rawQuery) => {
      const query = rawQuery.trim();
      if (!query) return;
      const yqMode = query.startsWith("?");
      const columnType = yqMode ? "yq" : "search";
      const columnParam = yqMode ? query.slice(1).trim() : query;
      if (!columnParam) return;
      const namePrefix = yqMode ? "YQ" : t("Search");
      const shortQuery =
        columnParam.length > 40 ? `${columnParam.slice(0, 39)}...` : columnParam;
      open({
        resourceKey: `${columnType}:${columnParam}`,
        column: {
          ...createColumn(0, 0, columnType),
          columnParam,
          name: `${namePrefix}: ${shortQuery}`,
          maxStatuses: 100,
        },
      });
    },
    openThreadPane: (status) => {
      const statusId = status.originalStatusId || status.id;
      if (!statusId || !status.serverDomain) return;
      const columnParam = JSON.stringify({
        statusId,
        serverDomain: status.serverDomain,
        sourceAcct: status.sourceAcct,
      });
      open({
        resourceKey: `thread:${columnParam}`,
        column: {
          ...createColumn(0, 0, "thread"),
          columnParam,
          name: t("Thread"),
          maxStatuses: 240,
        },
      });
    },
    openAirContextPane: (status) => {
      const statusId = status.originalStatusId || status.id;
      const accountId = status.notificationAccountId;
      if (!statusId || !status.serverDomain || !accountId) return;
      const columnParam = JSON.stringify({
        statusId,
        serverDomain: status.serverDomain,
        accountId,
        accountAcct: status.notificationAcct,
        sourceAcct: status.sourceAcct,
      });
      open({
        resourceKey: `airContext:${columnParam}`,
        column: {
          ...createColumn(0, 0, "airContext"),
          columnParam,
          name: t("AIR context"),
          maxStatuses: 2,
        },
      });
    },
    openUserPane: (status) => {
      const target: UserProfileTarget = {
        accountId: status.accountId,
        serverDomain: status.serverDomain,
        sourceAcct: status.sourceAcct,
        acct: status.acct,
        displayName: status.displayName,
        avatar: status.avatar,
      };
      open(
        {
          resourceKey: `profile:${target.serverDomain}:${target.accountId}:${target.sourceAcct ?? "cache"}`,
          column: {
            ...createColumn(0, 0, "profile"),
            name: target.acct,
            maxStatuses: 80,
            profile: target,
          },
          updateExisting: (current) => ({
            name: target.acct || current.name,
            profile: {
              ...target,
              acct: target.acct || current.profile?.acct || "",
              displayName: target.displayName || current.profile?.displayName || "",
              avatar: target.avatar || current.profile?.avatar || "",
            },
          }),
        },
        { load: false },
      );
    },
  };
}
