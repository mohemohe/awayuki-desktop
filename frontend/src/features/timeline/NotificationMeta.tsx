import { MessageCircle, Repeat2, Star } from "lucide-react";
import { t } from "../../i18n";
import type { TimelineStatus } from "../../types/app";
import { formatTime } from "../../utils/format";
import { Avatar } from "../../components/common/Avatar";
import { CustomEmojiText } from "../../components/common/CustomEmoji";
import {
  notificationKindIsReblog,
  semanticNotificationKind,
} from "../../domain/notification";

export function NotificationMeta({
  status,
  onOpenUser,
}: {
  status: TimelineStatus;
  onOpenUser: (status: TimelineStatus) => void;
}) {
  if (!status.notificationLabel) return null;

  const Icon = notificationMetaIcon(status.notificationKind);
  const boostCreatedAt =
    status.originalCreatedAt && notificationKindIsReblog(status.notificationKind)
      ? status.createdAt
      : null;
  const notificationUserStatus = status.notificationAccountId
    ? {
        ...status,
        accountId: status.notificationAccountId,
        acct: status.notificationAcct ?? status.notificationAccountId,
        displayName:
          status.notificationDisplayName ??
          status.notificationAcct ??
          status.notificationAccountId,
        avatar: status.notificationAvatar ?? "",
        accountEmojis: status.notificationAccountEmojis ?? [],
      }
    : null;
  const className =
    "mb-1 flex min-w-0 max-w-full items-center gap-1.5 text-xs font-semibold text-overlay0";
  const content = (
    <>
      <Icon className="h-3.5 w-3.5 shrink-0" />
      {status.notificationAvatar ? (
        <Avatar
          src={status.notificationAvatar}
          label={status.notificationLabel}
          size="xs"
        />
      ) : null}
      <span className="min-w-0 truncate">
        <CustomEmojiText
          text={status.notificationLabel}
          emojis={status.notificationAccountEmojis ?? []}
        />
      </span>
      {boostCreatedAt ? (
        <span className="shrink-0 font-normal text-overlay0">
          {formatTime(boostCreatedAt)}
        </span>
      ) : null}
    </>
  );

  if (!notificationUserStatus) {
    return <div className={className}>{content}</div>;
  }
  return (
    <button
      type="button"
      className={`${className} hover:text-blue`}
      onClick={(event) => {
        event.stopPropagation();
        onOpenUser(notificationUserStatus);
      }}
      title={t("Open profile")}
    >
      {content}
    </button>
  );
}

function notificationMetaIcon(kind?: string | null) {
  const semanticKind = semanticNotificationKind(kind);
  if (semanticKind === "reblog") return Repeat2;
  if (semanticKind === "favourite") return Star;
  return MessageCircle;
}
