import type {
  AccountListSummary,
  AccountRelationshipSummary,
  AppSnapshot,
  CustomEmojiSummary,
  DeleteStatusRequest,
  EditStatusRequest,
  HashtagSuggestion,
  MentionSuggestion,
  SaveColumnsRequest,
  SessionCapabilities,
  StatusActionRequest,
  TimelineRequest,
  TimelineStatus,
} from "../types/app";
import { filenameFromPath } from "../utils/format";
import type { IpcCommandName } from "./generated/contract";

/** Kept exhaustive by the contract test whenever the generated map changes. */
export const MOCK_IMPLEMENTED_COMMANDS = [
  "app_snapshot",
  "start_runtime_initialization",
  "retry_runtime_initialization",
  "account_summaries",
  "account_lists",
  "login_with_instance_domain",
  "login_with_bluesky_app_password",
  "load_timeline",
  "load_more_timeline",
  "refresh_timeline",
  "status_thread",
  "air_context",
  "account_profile",
  "account_timeline",
  "account_follow_action",
  "notification_muted_accounts",
  "set_account_notification_mute",
  "post_status",
  "begin_compose_media_upload",
  "append_compose_media_upload",
  "finish_compose_media_upload",
  "cancel_compose_media_upload",
  "claim_dropped_media_path",
  "upload_compose_media_path",
  "autocomplete_mentions",
  "autocomplete_hashtags",
  "custom_emojis",
  "edit_own_status",
  "delete_own_status",
  "vote_poll",
  "switch_active_account",
  "logout_account",
  "save_settings",
  "translate_status_text",
  "save_columns",
  "explain_custom_timeline",
  "vacuum_database",
  "clear_status_cache",
  "status_bar_snapshot",
  "status_action",
  "download_media",
  "open_status_url",
  "create_sidecar_webview",
  "navigate_sidecar_webview",
  "reload_sidecar_webview",
  "close_sidecar_webview",
  "scroll_sidecar_webview_to_top",
  "inject_sidecar_user_style",
  "open_log_file",
  "diagnostics_snapshot",
  "support_bundle",
] as const satisfies readonly IpcCommandName[];

export class UnsupportedMockCommandError extends Error {
  constructor(readonly command: IpcCommandName) {
    super(`Mock IPC command is not implemented: ${command}`);
    this.name = "UnsupportedMockCommandError";
  }
}

const activityPubCapabilities: SessionCapabilities = {
  protocol: "activityPub",
  timelines: {
    home: true,
    public: true,
    local: true,
    lists: true,
    hashtags: true,
    notifications: true,
    bookmarks: true,
    favourites: true,
  },
  status: {
    favourite: true,
    reblog: true,
    bookmark: true,
    vote: true,
    edit: true,
    delete: true,
  },
  relationship: { follow: true, mute: true, block: true },
  compose: {
    mediaUpload: true,
    poll: true,
    quote: true,
    maxMediaAttachments: 4,
    maxCharacters: 500,
  },
  streaming: true,
};

const atProtoCapabilities: SessionCapabilities = {
  ...activityPubCapabilities,
  protocol: "atProto",
  timelines: {
    ...activityPubCapabilities.timelines,
    public: false,
    local: false,
    favourites: false,
  },
  status: { ...activityPubCapabilities.status, vote: false },
  compose: {
    mediaUpload: false,
    poll: false,
    quote: true,
    maxMediaAttachments: 0,
    maxCharacters: 300,
  },
};

const mockUploads = new Map<string, { written: number; total: number; filename: string }>();

export async function mockInvoke<T>(
  command: IpcCommandName,
  args?: Record<string, unknown>,
): Promise<T> {
  if (command === "app_snapshot") return mockSnapshot as T;
  if (command === "start_runtime_initialization") return undefined as T;
  if (command === "retry_runtime_initialization") return undefined as T;
  if (command === "explain_custom_timeline") {
    return [{ id: 0, parent: 0, detail: "SCAN statuses" }] as T;
  }
  if (command === "switch_active_account") {
    const acct = args?.acct as string | undefined;
    if (acct) {
      mockSnapshot.activeAcct = acct;
      mockSnapshot.accounts = mockSnapshot.accounts.map((account) => ({
        ...account,
        isActive: account.acct === acct,
      }));
    }
    return mockSnapshot as T;
  }
  if (
    command === "login_with_instance_domain" ||
    command === "login_with_bluesky_app_password"
  ) {
    const acct =
      command === "login_with_bluesky_app_password"
        ? "newuser.bsky.social"
        : "newuser@example.social";
    mockSnapshot.accounts = mockSnapshot.accounts.map((account) => ({
      ...account,
      isActive: false,
    }));
    mockSnapshot.accounts.push({
      acct,
      serverDomain:
        command === "login_with_bluesky_app_password"
          ? "bsky.social"
          : "example.social",
      accountId: `${mockSnapshot.accounts.length + 1}`,
      displayName: "New User",
      avatar: "",
      isActive: true,
      serverKind:
        command === "login_with_bluesky_app_password" ? "bluesky" : "mastodon",
      characterLimit: command === "login_with_bluesky_app_password" ? 300 : 500,
      capabilities:
        command === "login_with_bluesky_app_password"
          ? atProtoCapabilities
          : activityPubCapabilities,
    });
    mockSnapshot.activeAcct = acct;
    return mockSnapshot as T;
  }
  if (command === "account_summaries") return mockSnapshot.accounts as T;
  if (command === "autocomplete_mentions") {
    const request = args?.request as { query?: string; limit?: number } | undefined;
    const query = (request?.query ?? "").replace(/^@/, "").toLowerCase();
    const limit = request?.limit ?? 8;
    return mockMentionSuggestions
      .filter((suggestion) => suggestion.acct.toLowerCase().startsWith(query))
      .slice(0, limit) as T;
  }
  if (command === "autocomplete_hashtags") {
    const request = args?.request as { query?: string; limit?: number } | undefined;
    const query = (request?.query ?? "").replace(/^#/, "").toLowerCase();
    const limit = request?.limit ?? 8;
    return mockHashtagSuggestions
      .filter((suggestion) => suggestion.name.toLowerCase().startsWith(query))
      .slice(0, limit) as T;
  }
  if (command === "account_lists") {
    const request = args?.request as { acct?: string } | undefined;
    const prefix = request?.acct?.includes("bsky") ? "Bluesky" : "Main";
    return ([
      { id: `${prefix.toLowerCase()}-friends`, title: `${prefix} Friends` },
      { id: `${prefix.toLowerCase()}-dev`, title: `${prefix} Dev` },
      { id: `${prefix.toLowerCase()}-news`, title: `${prefix} News` },
    ] satisfies AccountListSummary[]) as T;
  }
  if (command === "status_bar_snapshot") {
    return {
      statusCount: mockSnapshot.database.statusCount,
      recentStatusCount: 9,
      uptimeSeconds: 20_683,
    } as T;
  }
  if (command === "load_timeline" || command === "refresh_timeline") {
    const request = args?.request as TimelineRequest | undefined;
    const label =
      request?.columnType === "notification"
        ? "Notification"
        : request?.columnType === "public"
          ? "Federated"
          : request?.columnType === "search"
            ? `Search: ${request.columnParam ?? ""}`
            : request?.columnType === "yq"
              ? `YQ: ${request.columnParam ?? ""}`
              : request?.columnType === "user_bookmarks"
                ? "Bookmarks"
                : request?.columnType === "favourites"
                  ? "Favorites"
                : "Home";
    return mockStatuses(label, request?.offset ?? 0, request?.limit ?? 8) as T;
  }
  if (command === "load_more_timeline") {
    const request = args?.request as TimelineRequest | undefined;
    return {
      statuses: mockStatuses(
        request?.columnType ?? "Home",
        request?.offset ?? 0,
        request?.limit ?? 8,
      ),
      hasMore: true,
    } as T;
  }
  if (command === "status_thread") {
    return mockStatuses("Thread", 0, 12).map((status, index) => ({
      ...status,
      id: `thread-${index}`,
      originalStatusId: `thread-${index}`,
      statusIdentity: {
        protocol: "activityPub",
        serverDomain: status.serverDomain,
        canonicalUri: `https://${status.serverDomain}/statuses/thread-${index}`,
        remoteId: `thread-${index}`,
      },
      inReplyToId: index === 0 ? null : `thread-${Math.max(0, index - 1)}`,
      inReplyToAccountId: index === 0 ? null : "account-0",
    })) as T;
  }
  if (command === "air_context") {
    return mockStatuses("AIR context", 0, 2).map((status, index) => ({
      ...status,
      id: `air-context-${index}`,
      originalStatusId: `air-context-${index}`,
      statusIdentity: {
        protocol: "activityPub",
        serverDomain: status.serverDomain,
        canonicalUri: `https://${status.serverDomain}/statuses/air-context-${index}`,
        remoteId: `air-context-${index}`,
      },
      content:
        index === 0
          ? "<p>Notification target post</p>"
          : "<p>Notification source user's next post</p>",
    })) as T;
  }
  if (command === "post_status") return mockStatuses("Home")[0] as T;
  if (command === "account_profile") {
    const request = args?.request as
      | { accountId?: string; serverDomain?: string }
      | undefined;
    return {
      id: request?.accountId ?? "account-0",
      serverDomain: request?.serverDomain ?? "example.social",
      username: "mohemohe",
      acct: "mohemohe@example.social",
      url: "https://example.social/@mohemohe",
      displayName: "今昼重森",
      note: "石橋とLLMを叩くのが得意です。",
      avatar: "https://placehold.co/96x96/89b4fa/11111b?text=A",
      header: "https://placehold.co/640x220/45475a/cdd6f4?text=Cover",
      fields: [
        { name: "Blog", value: "https://mohemohe.dev" },
        { name: "GitHub", value: "https://github.com/mohemohe" },
      ],
      accountEmojis: [],
      statusesCount: 230900,
      followingCount: 378,
      followersCount: 477,
      isSelf: false,
      relationship: {
        following: false,
        followedBy: true,
        requested: false,
        blocking: false,
        muting: false,
      },
      notificationMuted: false,
    } as T;
  }
  if (command === "account_timeline") return mockStatuses("Profile") as T;
  if (command === "account_follow_action") {
    const request = args?.request as { action?: string } | undefined;
    return {
      following: request?.action === "follow",
      followedBy: true,
      requested: false,
      blocking: request?.action === "block",
      muting: request?.action === "mute",
    } satisfies AccountRelationshipSummary as T;
  }
  if (command === "set_account_notification_mute") {
    const request = args?.request as { muted?: boolean } | undefined;
    return Boolean(request?.muted) as T;
  }
  if (command === "notification_muted_accounts") {
    return [
      {
        accountId: "account-0",
        serverDomain: "example.social",
        acct: "mohemohe@example.social",
        displayName: "今昼重森",
        avatar: "https://placehold.co/96x96/89b4fa/11111b?text=A",
        createdAt: new Date(Date.now() - 86_400_000).toISOString(),
        updatedAt: new Date().toISOString(),
      },
    ] as T;
  }
  if (command === "begin_compose_media_upload") {
    const request = args?.request as
      | { filename?: string; size?: number }
      | undefined;
    const uploadId = crypto.randomUUID();
    mockUploads.set(uploadId, {
      written: 0,
      total: request?.size ?? 0,
      filename: request?.filename ?? "upload",
    });
    return { uploadId } as T;
  }
  if (command === "append_compose_media_upload") {
    const request = args?.request as
      | { uploadId?: string; data?: number[] }
      | undefined;
    const upload = request?.uploadId
      ? mockUploads.get(request.uploadId)
      : undefined;
    if (!upload) throw new Error("Unknown mock upload");
    upload.written += request?.data?.length ?? 0;
    return { written: upload.written, total: upload.total } as T;
  }
  if (command === "finish_compose_media_upload") {
    const request = args?.request as { uploadId?: string } | undefined;
    const upload = request?.uploadId
      ? mockUploads.get(request.uploadId)
      : undefined;
    if (!upload) throw new Error("Unknown mock upload");
    if (request?.uploadId) mockUploads.delete(request.uploadId);
    return {
      id:
        typeof crypto !== "undefined" && "randomUUID" in crypto
          ? crypto.randomUUID()
          : `${Date.now()}`,
      media_type: "image",
      preview_url:
        "https://placehold.co/320x240/313244/cdd6f4?text=Attached+media",
      url: null,
      remote_url: null,
      description: upload.filename,
    } as T;
  }
  if (command === "cancel_compose_media_upload") {
    const request = args?.request as { uploadId?: string } | undefined;
    if (request?.uploadId) mockUploads.delete(request.uploadId);
    return undefined as T;
  }
  if (command === "claim_dropped_media_path") {
    return { capability: crypto.randomUUID() } as T;
  }
  if (command === "upload_compose_media_path") {
    const request = args?.request as { path?: string } | undefined;
    return {
      id:
        typeof crypto !== "undefined" && "randomUUID" in crypto
          ? crypto.randomUUID()
          : `${Date.now()}`,
      media_type: "image",
      preview_url:
        "https://placehold.co/320x240/313244/cdd6f4?text=Attached+media",
      url: null,
      remote_url: null,
      description: request?.path ? filenameFromPath(request.path) : null,
    } as T;
  }
  if (command === "custom_emojis") return mockCustomEmojis as T;
  if (command === "open_status_url") return undefined as T;
  if (command === "create_sidecar_webview") return undefined as T;
  if (command === "navigate_sidecar_webview") return undefined as T;
  if (command === "reload_sidecar_webview") return undefined as T;
  if (command === "close_sidecar_webview") return undefined as T;
  if (command === "scroll_sidecar_webview_to_top") return undefined as T;
  if (command === "inject_sidecar_user_style") return undefined as T;
  if (command === "open_log_file") return undefined as T;
  if (command === "download_media") return undefined as T;
  if (command === "logout_account") {
    const acct = args?.acct as string | undefined;
    if (acct) {
      mockSnapshot.accounts = mockSnapshot.accounts.filter(
        (account) => account.acct !== acct,
      );
      if (mockSnapshot.activeAcct === acct)
        mockSnapshot.activeAcct = mockSnapshot.accounts[0]?.acct ?? null;
    }
    return mockSnapshot as T;
  }
  if (command === "status_action") {
    const request = args?.request as StatusActionRequest | undefined;
    const item = { ...mockStatuses("Home")[0] };
    if (request?.action.includes("favourite"))
      item.favourited = request.action === "favourite";
    if (request?.action.includes("reblog"))
      item.reblogged = request.action === "reblog";
    if (request?.action.includes("bookmark"))
      item.bookmarked = request.action === "bookmark";
    return item as T;
  }
  if (command === "vote_poll") {
    return {
      id: "poll-1",
      expiresAt: new Date(Date.now() + 23 * 60 * 60 * 1000).toISOString(),
      expired: false,
      multiple: false,
      votesCount: 1,
      votersCount: 1,
      voted: true,
      ownVotes: [0],
      emojis: [],
      options: [
        { title: "1", votesCount: 1 },
        { title: "2", votesCount: 0 },
      ],
    } as T;
  }
  if (command === "edit_own_status") {
    const request = args?.request as EditStatusRequest | undefined;
    const item = { ...mockStatuses("Home")[1] };
    item.content = `<p>${request?.status ?? "Edited post"}</p>`;
    return item as T;
  }
  if (command === "delete_own_status") {
    const request = args?.request as DeleteStatusRequest | undefined;
    void request;
    return undefined as T;
  }
  if (command === "save_settings") {
    const request = args?.request as
      | { key?: keyof AppSnapshot["settings"] | string; value?: unknown }
      | undefined;
    const key = mockSettingsKey(request?.key);
    if (key && key in mockSnapshot.settings) {
      mockSnapshot.settings = {
        ...mockSnapshot.settings,
        [key]: request?.value,
      };
    }
    return mockSnapshot.settings as T;
  }
  if (command === "translate_status_text") {
    const request = args?.request as
      | {
          text?: string;
          sourceLanguage?: string | null;
          targetLanguage?: string;
          translationEngine?: string;
        }
      | undefined;
    return {
      text: `Translated: ${request?.text ?? ""}`,
      sourceLanguage: request?.sourceLanguage ?? "en",
      targetLanguage: request?.targetLanguage ?? "ja",
    } as T;
  }
  if (command === "save_columns") {
    const request = args?.request as SaveColumnsRequest | undefined;
    if (request?.columns) mockSnapshot.columns = request.columns;
    return mockSnapshot as T;
  }
  if (command === "vacuum_database" || command === "clear_status_cache")
    return mockSnapshot.database as T;
  if (command === "diagnostics_snapshot") return mockDiagnosticsSnapshot() as T;
  if (command === "support_bundle") {
    const request = args?.request as
      | { frontend?: Record<string, number> }
      | undefined;
    return {
      schemaVersion: 1,
      generatedAt: new Date().toISOString(),
      environment: {
        appVersion: mockSnapshot.version,
        databaseSchemaVersion: 20,
        persistence: "sqlite_only_portable",
      },
      backend: mockDiagnosticsSnapshot(),
      frontend: request?.frontend ?? mockFrontendHealthSnapshot(),
      recentEvents: [],
    } as T;
  }
  throw new UnsupportedMockCommandError(command);
}

function mockDiagnosticsSnapshot() {
  return {
    schemaVersion: 1,
    activeOperations: 0,
    completedOperations: 0,
    failedOperations: 0,
    apiRequests: 0,
    dbTransactions: 0,
    dbBusyErrors: 0,
    cacheEntries: 0,
    stream: {
      queueDepth: 0,
      maxQueueDepth: 0,
      coalesced: 0,
      dropped: 0,
      resyncs: 0,
      resyncRequired: false,
    },
    droppedLogRecords: 0,
    rollingEventCount: 0,
  };
}

function mockFrontendHealthSnapshot() {
  return {
    activeOperations: 0,
    completedOperations: 0,
    failedOperations: 0,
    streamSequenceGaps: 0,
    streamResyncs: 0,
    pendingStreamEvents: 0,
  };
}

function mockSettingsKey(key?: string) {
  if (key === "account_source_colors") return "accountSourceColors";
  if (key === "bluesky_fetch") return "blueskyFetch";
  if (key === "sidecars") return "sidecars";
  if (key === "preset_visibility") return "presetVisibility";
  if (key === "notification_suppression") return "notificationSuppression";
  return key;
}

const mockSnapshotTemplate: AppSnapshot = {
  version: "0.1.0",
  activeAcct: "mohemohe@example.social",
  accounts: [
    {
      acct: "mohemohe@example.social",
      serverDomain: "example.social",
      accountId: "1",
      displayName: "今昼重森",
      avatar: "",
      isActive: true,
      serverKind: "mastodon",
      characterLimit: 500,
      capabilities: activityPubCapabilities,
    },
    {
      acct: "mohemohe.bsky.social",
      serverDomain: "bsky.social",
      accountId: "did:plc:example",
      displayName: "mohemohe",
      avatar: "",
      isActive: false,
      serverKind: "bluesky",
      characterLimit: 300,
      capabilities: atProtoCapabilities,
      rateLimit: {
        limit: 3000,
        remaining: 2996,
        used: 4,
        resetInSeconds: 175,
        observedAgoSeconds: 33,
        policy: "3000;w=300",
        usedFraction: 4 / 3000,
      },
    },
  ],
  columns: [
    {
      id: "home",
      columnType: "home",
      name: "Home",
      maxStatuses: 40,
      paneIndex: 0,
      position: 0,
    },
    {
      id: "public",
      columnType: "public",
      name: "Federated",
      maxStatuses: 40,
      paneIndex: 0,
      position: 1,
    },
    {
      id: "ff14",
      columnType: "hashtag",
      columnParam: "FF14",
      name: "FF14",
      maxStatuses: 40,
      paneIndex: 1,
      position: 0,
    },
    {
      id: "notification",
      columnType: "notification",
      name: "Notification",
      maxStatuses: 40,
      paneIndex: 1,
      position: 1,
    },
  ],
  settings: {
    appearance: {
      avatar_shape: "Circle",
      font_size: "Medium",
      cw_behavior: "Hide",
      nsfw_behavior: "Hide",
      display_mode: "StarryEyes",
    },
    performance: {
      mention_source: "SQLite",
      hashtag_source: "SQLite",
      timeline_renderer: "VirtualList",
    },
    confirmation: {
      confirm_boost: true,
      confirm_favourite: true,
      confirm_follow: true,
      confirm_unfollow: true,
      jumbomoji_enabled: false,
      show_status_application: false,
      status_application_position: "AboveActions",
      media_source: "Local",
      translate_enabled: false,
      auto_translate_enabled: false,
      translation_engine: "TranslationFramework",
    },
    blueskyFetch: { intervals_by_acct: {} },
    sidecars: {
      entries: [],
      mainViewIndex: 0,
    },
    accountSourceColors: {
      "mohemohe@example.social": "Mauve",
      "mohemohe.bsky.social": "Sapphire",
    },
    presetVisibility: {
      entries: [{ keyword: "notification", visibility: "Unlisted" }],
    },
    debug: { logging_enabled: false, log_level: "Info" },
    notificationSuppression: { suppressed_accts: [] },
  },
  database: {
    path: "/Users/mohemohe/Library/Application Support/awayuki/awayuki.db",
    size: "42.1 MB",
    statusCount: 577192,
    recentStatusCount: 412,
    accountCount: 18422,
  },
};

export function createMockFixture(): AppSnapshot {
  return structuredClone(mockSnapshotTemplate);
}

let mockSnapshot = createMockFixture();

export function resetMockFixture() {
  mockSnapshot = createMockFixture();
  mockUploads.clear();
}

const mockMentionSuggestions: MentionSuggestion[] = [
  {
    acct: "mohemohe@example.social",
    displayName: "今昼重森",
    avatar: "https://placehold.co/96x96/89b4fa/11111b?text=A",
  },
  {
    acct: "mona@example.social",
    displayName: "Mona",
    avatar: "https://placehold.co/96x96/a6e3a1/11111b?text=M",
  },
  {
    acct: "misskey_dev@example.social",
    displayName: "Misskey Dev",
    avatar: "https://placehold.co/96x96/f9e2af/11111b?text=D",
  },
];

const mockHashtagSuggestions: HashtagSuggestion[] = [
  { name: "FF14" },
  { name: "fediverse" },
  { name: "frontend" },
  { name: "fedi_dev" },
];

const mockCustomEmojis: CustomEmojiSummary[] = [
  {
    shortcode: "awayuki",
    url: "https://placehold.co/32x32/89b4fa/11111b?text=A",
    staticUrl: "https://placehold.co/32x32/89b4fa/11111b?text=A",
    category: "Awayuki",
  },
  {
    shortcode: "blob_aww",
    url: "https://placehold.co/32x32/a6e3a1/11111b?text=B",
    staticUrl: "https://placehold.co/32x32/a6e3a1/11111b?text=B",
    category: "Blob",
  },
  {
    shortcode: "cat_think",
    url: "https://placehold.co/32x32/f9e2af/11111b?text=C",
    staticUrl: "https://placehold.co/32x32/f9e2af/11111b?text=C",
    category: "Reaction",
  },
];

function mockStatuses(
  label: string,
  offset = 0,
  limit = 8,
): TimelineStatus[] {
  const length = offset >= 160 ? 0 : Math.min(limit, 40);
  return Array.from({ length }, (_, index) => {
    const itemIndex = offset + index;
    return {
      id: `${label}-${itemIndex}`,
      originalStatusId: `${label}-${itemIndex}`,
      statusIdentity: {
        protocol: "activityPub",
        serverDomain: "example.social",
        canonicalUri: `https://example.social/statuses/${label}-${itemIndex}`,
        remoteId: `${label}-${itemIndex}`,
      },
      sourceAcct:
        itemIndex % 2 === 0
          ? "mohemohe@example.social"
          : "mohemohe.bsky.social",
      accountId: itemIndex % 2 === 0 ? "account-0" : "1",
      serverDomain: "example.social",
      uri: `https://example.social/statuses/${label}-${itemIndex}`,
      url: `https://example.social/@demo/${label}-${itemIndex}`,
      displayName: itemIndex % 2 === 0 ? "Giraffe Beer" : "今昼重森",
      acct:
        itemIndex % 2 === 0
          ? "@giraffe_beer@example.social"
          : "@mohemohe@example.social",
      avatar: "",
      createdAt: new Date(Date.now() - itemIndex * 600000).toISOString(),
      inReplyToId: null,
      inReplyToAccountId: null,
      content:
        itemIndex % 3 === 0
          ? '<p>ハンバーグちゃんがかわいいね :awayuki:</p><p><a href="#">#Awayuki</a> のTauri移行プレビューです。</p>'
          : "<p>FF14 prep notes. This keeps the existing multi-column layout aligned with the Catppuccin palette.</p>",
      spoilerText: "",
      language: itemIndex % 3 === 0 ? "ja" : "en",
      applicationName:
        itemIndex % 2 === 0 ? "Twitter Web App" : "Awayuki Desktop",
      reblogsCount: itemIndex * 2,
      favouritesCount: itemIndex + 1,
      repliesCount: itemIndex % 4,
      visibility: "public",
      sensitive: itemIndex === 0,
      favourited: itemIndex === 1,
      reblogged: itemIndex === 2,
      bookmarked: itemIndex === 3,
      media:
        itemIndex === 0
          ? [
              {
                id: "m1",
                preview_url:
                  "https://placehold.co/600x360/313244/cdd6f4?text=Awayuki",
                description: "Preview",
                blurhash: "LEHV6nWB2yk8pyo0adR*.7kCMdnj",
              },
            ]
          : [],
      poll:
        itemIndex === 0
          ? {
              id: "poll-1",
              expiresAt: new Date(
                Date.now() + 23 * 60 * 60 * 1000,
              ).toISOString(),
              expired: false,
              multiple: false,
              votesCount: 0,
              votersCount: 0,
              voted: false,
              ownVotes: [],
              emojis: [],
              options: [
                { title: "1", votesCount: 0 },
                { title: "2", votesCount: 0 },
              ],
            }
          : null,
      emojis:
        itemIndex === 0
          ? [
              {
                shortcode: "awayuki",
                url: "https://placehold.co/32x32/89b4fa/11111b?text=A",
                staticUrl: "https://placehold.co/32x32/89b4fa/11111b?text=A",
              },
            ]
          : [],
      accountEmojis: [],
      notificationId:
        label === "Notification" ? `notification-${itemIndex}` : null,
      notificationKind: label === "Notification" ? "favourite" : null,
      notificationLabel: label === "Notification" ? "toto favourited" : null,
      notificationAvatar: label === "Notification" ? "" : null,
      notificationAccountId: label === "Notification" ? "account-2" : null,
      notificationAcct: label === "Notification" ? "@toto@example.social" : null,
      notificationDisplayName: label === "Notification" ? "toto" : null,
      notificationAccountEmojis: [],
    };
  });
}
