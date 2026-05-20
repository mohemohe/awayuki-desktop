import type {
  AccountProfileSummary,
  ConfirmationDialogRequest,
  ConfirmationSettings,
  TimelineStatus,
} from "../types/app";
import { t } from "../i18n";

export type ConfirmationRequester = (
  request: ConfirmationDialogRequest,
) => Promise<boolean>;

export function displaySubject(status: TimelineStatus) {
  return status.displayName || status.acct || t("this user");
}

export function profileSubject(profile: AccountProfileSummary) {
  return profile.displayName || profile.acct || profile.username || t("this user");
}

export async function confirmStatusAction(
  settings: ConfirmationSettings | undefined,
  requestConfirmation: ConfirmationRequester,
  status: TimelineStatus,
  action: string,
) {
  if (action === "reblog") {
    if (!settings?.confirm_boost) return true;
    const subject = displaySubject(status);
    return requestConfirmation({
      title: t("Confirm boost"),
      message: t("Boost this post by {subject}?", { subject }),
      confirmLabel: t("Boost"),
    });
  }

  if (action === "favourite") {
    if (!settings?.confirm_favourite) return true;
    const subject = displaySubject(status);
    return requestConfirmation({
      title: t("Confirm favorite"),
      message: t("Favorite this post by {subject}?", { subject }),
      confirmLabel: t("Favorite"),
    });
  }

  return true;
}

export async function confirmFollowAction(
  settings: ConfirmationSettings | undefined,
  requestConfirmation: ConfirmationRequester,
  profile: AccountProfileSummary,
  action: "follow" | "unfollow",
) {
  if (action === "follow") {
    if (!settings?.confirm_follow) return true;
    return requestConfirmation({
      title: t("Confirm follow"),
      message: t("Follow {subject}?", { subject: profileSubject(profile) }),
      confirmLabel: t("Follow"),
    });
  }

  if (!settings?.confirm_unfollow) return true;
  return requestConfirmation({
    title: t("Confirm unfollow"),
    message: t("Unfollow {subject}?", { subject: profileSubject(profile) }),
    confirmLabel: t("Unfollow"),
    danger: true,
  });
}
