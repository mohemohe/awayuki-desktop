export type AccountSummary = {
  acct: string;
  serverDomain: string;
  accountId: string;
  displayName: string;
  avatar: string;
  isActive: boolean;
  serverKind: string;
  characterLimit: number;
  rateLimit?: AccountRateLimitSummary | null;
};

export type AccountRateLimitSummary = {
  limit: number;
  remaining: number;
  used: number;
  resetInSeconds: number;
  observedAgoSeconds: number;
  policy?: string | null;
  usedFraction: number;
};

export type AccountListSummary = {
  id: string;
  title: string;
};

export type AccountSourceColor =
  | "Transparent"
  | "Mauve"
  | "Red"
  | "Peach"
  | "Yellow"
  | "Green"
  | "Sapphire"
  | "Lavender";

export type ColumnSummary = {
  id: string;
  columnType: string;
  columnParam?: string | null;
  name: string;
  maxStatuses: number;
  paneIndex: number;
  position: number;
  accountAcct?: string | null;
  displayFilter?: TimelineDisplayFilter | null;
  dynamic?: boolean;
  profile?: UserProfileTarget;
};

export type TimelineDisplayFilter = {
  enabled: boolean;
  excludeBoosts: boolean;
  excludeMedia: boolean;
  includeMedia: boolean;
};

export type PaneGroup = {
  paneIndex: number;
  tabs: ColumnSummary[];
};

export type DbSummary = {
  path: string;
  size: string;
  statusCount: number;
  recentStatusCount: number;
  accountCount: number;
};

export type StatusBarSnapshot = {
  statusCount: number;
  recentStatusCount: number;
  uptimeSeconds: number;
  fetchedAt: number;
};

export type AppearanceSettings = {
  avatar_shape: "Square" | "Circle" | "Rounded";
  font_size: "Small" | "Medium" | "Large";
  cw_behavior: "Hide" | "AlwaysExpand";
  nsfw_behavior: "Hide" | "AlwaysShow";
  display_mode: "StarryEyes" | "Mystique";
};

export type PerformanceSettings = {
  mention_source: "Server" | "SQLite";
  hashtag_source: "Server" | "SQLite";
  timeline_renderer: "List" | "VirtualList";
};

export type ConfirmationSettings = {
  confirm_boost: boolean;
  confirm_favourite: boolean;
  confirm_follow: boolean;
  confirm_unfollow: boolean;
  jumbomoji_enabled?: boolean;
  show_status_application?: boolean;
  media_source: "Local" | "Remote";
  translate_enabled: boolean;
  auto_translate_enabled: boolean;
  translation_engine: "FoundationModel" | "TranslationFramework";
};

export type BlueskyFetchSettings = {
  intervals_by_acct?: Record<string, number>;
  interval_seconds?: number;
};

export type SidecarEntry = {
  id: string;
  name: string;
  url: string;
  userStyleEnabled: boolean;
  userStyle: string;
  width: number;
};

export type SidecarSettings = {
  entries: SidecarEntry[];
  mainViewIndex: number;
};

export type ConfirmationDialogRequest = {
  title: string;
  message: string;
  confirmLabel: string;
  danger?: boolean;
};

export type ConfirmationDialogState = ConfirmationDialogRequest & {
  id: string;
};

export type PresetVisibilitySettings = {
  entries: Array<{
    keyword: string;
    visibility: "Public" | "Unlisted" | "Private" | "Direct";
  }>;
};

export type DebugSettings = {
  logging_enabled: boolean;
  log_level: "Error" | "Warn" | "Info" | "Debug" | "Trace";
};

export type SettingsSnapshot = {
  appearance: AppearanceSettings;
  performance: PerformanceSettings;
  confirmation: ConfirmationSettings;
  blueskyFetch: BlueskyFetchSettings;
  sidecars: SidecarSettings;
  accountSourceColors: Record<string, AccountSourceColor>;
  presetVisibility: PresetVisibilitySettings;
  debug: DebugSettings;
  notificationSuppression: { suppressed_accts: string[] };
};

export type AppSnapshot = {
  version: string;
  accounts: AccountSummary[];
  activeAcct?: string | null;
  columns: ColumnSummary[];
  settings: SettingsSnapshot;
  database: DbSummary;
};

export type MediaAttachment = {
  id: string;
  media_type?: string;
  type?: string;
  url?: string | null;
  preview_url?: string | null;
  remote_url?: string | null;
  description?: string | null;
  blurhash?: string | null;
};

export type UserProfileTarget = {
  accountId: string;
  serverDomain: string;
  acct: string;
  displayName: string;
  avatar: string;
};

export type AccountRelationshipSummary = {
  following: boolean;
  followedBy: boolean;
  requested: boolean;
  blocking: boolean;
  muting: boolean;
};

export type AccountProfileSummary = {
  id: string;
  serverDomain: string;
  username: string;
  acct: string;
  url?: string | null;
  displayName: string;
  note: string;
  avatar: string;
  header: string;
  fields: Array<{ name: string; value: string }>;
  accountEmojis: CustomEmojiSummary[];
  statusesCount: number;
  followingCount: number;
  followersCount: number;
  isSelf: boolean;
  relationship?: AccountRelationshipSummary | null;
  notificationMuted: boolean;
};

export type NotificationMutedAccountSummary = {
  accountId: string;
  serverDomain: string;
  acct: string;
  displayName: string;
  avatar: string;
  createdAt: string;
  updatedAt: string;
};

export type ComposeMediaAttachment = MediaAttachment & {
  filename: string;
  previewSrc: string;
  uploading?: boolean;
};

export type MentionSuggestion = {
  acct: string;
  displayName: string;
  avatar: string;
};

export type HashtagSuggestion = {
  name: string;
};

export type CustomEmojiSummary = {
  shortcode: string;
  url: string;
  staticUrl: string;
  category?: string | null;
};

export type PollOptionSummary = {
  title: string;
  votesCount?: number | null;
};

export type PollSummary = {
  id: string;
  expiresAt?: string | null;
  expired: boolean;
  multiple: boolean;
  votesCount: number;
  votersCount?: number | null;
  options: PollOptionSummary[];
  voted?: boolean | null;
  ownVotes?: number[] | null;
  emojis: CustomEmojiSummary[];
};

export type ComposePoll = {
  options: string[];
  multiple: boolean;
  expiresIn: number;
};

export type PostSubmitOptions = {
  mediaIds?: string[];
  spoilerText?: string;
  sensitive?: boolean;
  poll?: ComposePoll;
  inReplyToId?: string;
  quoteId?: string;
};

export type TimelineStatus = {
  id: string;
  originalStatusId: string;
  sourceAcct?: string | null;
  accountId: string;
  serverDomain: string;
  uri: string;
  url?: string | null;
  displayName: string;
  acct: string;
  avatar: string;
  createdAt: string;
  inReplyToId?: string | null;
  inReplyToAccountId?: string | null;
  content: string;
  spoilerText: string;
  language?: string | null;
  applicationName?: string | null;
  reblogsCount: number;
  favouritesCount: number;
  repliesCount: number;
  visibility: string;
  sensitive: boolean;
  favourited: boolean;
  reblogged: boolean;
  bookmarked: boolean;
  media: MediaAttachment[];
  poll?: PollSummary | null;
  emojis: CustomEmojiSummary[];
  accountEmojis: CustomEmojiSummary[];
  quoteId?: string | null;
  quoteOriginalUrl?: string | null;
  quote?: TimelineStatus | null;
  notificationId?: string | null;
  notificationLabel?: string | null;
  notificationAvatar?: string | null;
  notificationAccountId?: string | null;
  notificationAcct?: string | null;
  notificationDisplayName?: string | null;
  notificationAccountEmojis?: CustomEmojiSummary[];
};

export type MediaPreviewState = {
  status: TimelineStatus;
  media: MediaAttachment;
  src: string;
};

export type TimelineStreamEvent = {
  kind: "newStatus" | "statusUpdate" | "deleteStatus" | "newNotification";
  streamType: string;
  sourceAcct: string;
  serverDomain: string;
  status?: TimelineStatus | null;
  statusId?: string | null;
};

export type StartupSyncEvent = {
  kind: "bookmarkProgress" | "favouriteProgress" | "complete";
  message: string;
  acct?: string | null;
  page?: number | null;
  total?: number | null;
};

export type TimelineRequest = {
  columnType: string;
  columnParam?: string | null;
  limit?: number;
  offset?: number;
  maxStatusId?: string | null;
  sinceStatusId?: string | null;
  sinceServerDomain?: string | null;
  accountAcct?: string | null;
  displayFilter?: TimelineDisplayFilter | null;
};

export type TimelinePageResponse = {
  statuses: TimelineStatus[];
  hasMore: boolean;
};

export type SaveColumnsRequest = {
  columns: ColumnSummary[];
};

export type StatusActionRequest = {
  statusId: string;
  action: string;
};

export type VotePollRequest = {
  statusId: string;
  serverDomain: string;
  pollId: string;
  choices: number[];
};

export type EditStatusRequest = {
  statusId: string;
  serverDomain: string;
  accountId: string;
  status: string;
  visibility?: string | null;
  spoilerText?: string | null;
  sensitive?: boolean | null;
};

export type DeleteStatusRequest = {
  statusId: string;
  serverDomain: string;
  accountId: string;
};
