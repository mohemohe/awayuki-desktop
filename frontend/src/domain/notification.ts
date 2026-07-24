export type SemanticNotificationKind =
  | "mention"
  | "favourite"
  | "reblog"
  | "follow"
  | "poll"
  | "status"
  | "update"
  | "admin"
  | "unknown";

/** Provider spellings are normalized once; display labels never drive behavior. */
export function semanticNotificationKind(
  value?: string | null,
): SemanticNotificationKind {
  switch (value?.trim().toLowerCase()) {
    case "mention":
      return "mention";
    case "favourite":
    case "favorite":
    case "like":
      return "favourite";
    case "reblog":
    case "boost":
    case "repost":
      return "reblog";
    case "follow":
    case "follow_request":
      return "follow";
    case "poll":
      return "poll";
    case "status":
      return "status";
    case "update":
      return "update";
    case "admin.sign_up":
    case "admin.report":
      return "admin";
    default:
      return "unknown";
  }
}

export function notificationKindIsReblog(value?: string | null) {
  return semanticNotificationKind(value) === "reblog";
}
