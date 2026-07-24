import type { MessageId } from "../i18n";
import type { SessionCapabilities } from "../types/app";

export const configurableTimelineTypes = [
  "home",
  "public",
  "local",
  "notification",
  "bookmarks",
  "favourites",
  "hashtag",
  "list",
  "custom",
  "yq",
] as const;

export type ConfigurableTimelineType =
  (typeof configurableTimelineTypes)[number];

const accountBoundTimelineTypes = new Set(["local", "hashtag", "list"]);

/** Remote timelines whose source cannot be inferred from the mutation actor. */
export function timelineTypeRequiresAccount(value: string): boolean {
  return accountBoundTimelineTypes.has(value);
}

export type KnownTimelineType =
  | ConfigurableTimelineType
  | "search"
  | "user_bookmarks"
  | "thread"
  | "profile"
  | "airContext";

export type TimelineLoadStrategy =
  | "timeline"
  | "thread"
  | "airContext"
  | "profile";
export type TimelineStreamPolicy =
  | "home"
  | "public"
  | "local"
  | "notification"
  | "hashtag"
  | "list"
  | "none";
export type TimelinePagination = "api" | "local" | "none";
export type TimelineParameterEditor =
  | "none"
  | "text"
  | "list"
  | "sql"
  | "yq"
  | "internal";

type TimelineCapability =
  | keyof SessionCapabilities["timelines"]
  | "localDatabase"
  | "dynamicOnly";

export type TimelineDescriptor<T extends KnownTimelineType = KnownTimelineType> = {
  type: T;
  labelId: MessageId;
  /** Stable persisted English name. Never use the translation as identity. */
  defaultName: string;
  loadStrategy: TimelineLoadStrategy;
  pagination: TimelinePagination;
  streamPolicy: TimelineStreamPolicy;
  supportsDisplayFilter: boolean;
  parameterEditor: TimelineParameterEditor;
  capability: TimelineCapability;
  configurable: T extends ConfigurableTimelineType ? true : false;
};

const defineDescriptor = <T extends KnownTimelineType>(
  descriptor: TimelineDescriptor<T>,
) => descriptor;

export const timelineDescriptorRegistry = {
  home: defineDescriptor({
    type: "home",
    labelId: "timeline.home",
    defaultName: "Home",
    loadStrategy: "timeline",
    pagination: "local",
    streamPolicy: "home",
    supportsDisplayFilter: true,
    parameterEditor: "none",
    capability: "home",
    configurable: true,
  }),
  public: defineDescriptor({
    type: "public",
    labelId: "timeline.public",
    defaultName: "Federated",
    loadStrategy: "timeline",
    pagination: "local",
    streamPolicy: "public",
    supportsDisplayFilter: true,
    parameterEditor: "none",
    capability: "public",
    configurable: true,
  }),
  local: defineDescriptor({
    type: "local",
    labelId: "timeline.local",
    defaultName: "Local",
    loadStrategy: "timeline",
    pagination: "api",
    streamPolicy: "local",
    supportsDisplayFilter: true,
    parameterEditor: "none",
    capability: "local",
    configurable: true,
  }),
  notification: defineDescriptor({
    type: "notification",
    labelId: "timeline.notification",
    defaultName: "Notification",
    loadStrategy: "timeline",
    pagination: "local",
    streamPolicy: "notification",
    supportsDisplayFilter: false,
    parameterEditor: "none",
    capability: "notifications",
    configurable: true,
  }),
  bookmarks: defineDescriptor({
    type: "bookmarks",
    labelId: "timeline.bookmarks",
    defaultName: "Bookmarks",
    loadStrategy: "timeline",
    pagination: "local",
    streamPolicy: "none",
    supportsDisplayFilter: false,
    parameterEditor: "none",
    capability: "bookmarks",
    configurable: true,
  }),
  favourites: defineDescriptor({
    type: "favourites",
    labelId: "timeline.favourites",
    defaultName: "Favorites",
    loadStrategy: "timeline",
    pagination: "local",
    streamPolicy: "none",
    supportsDisplayFilter: false,
    parameterEditor: "none",
    capability: "favourites",
    configurable: true,
  }),
  hashtag: defineDescriptor({
    type: "hashtag",
    labelId: "timeline.hashtag",
    defaultName: "Hashtag",
    loadStrategy: "timeline",
    pagination: "api",
    streamPolicy: "hashtag",
    supportsDisplayFilter: true,
    parameterEditor: "text",
    capability: "hashtags",
    configurable: true,
  }),
  list: defineDescriptor({
    type: "list",
    labelId: "timeline.list",
    defaultName: "List",
    loadStrategy: "timeline",
    pagination: "api",
    streamPolicy: "list",
    supportsDisplayFilter: true,
    parameterEditor: "list",
    capability: "lists",
    configurable: true,
  }),
  custom: defineDescriptor({
    type: "custom",
    labelId: "timeline.custom",
    defaultName: "Custom",
    loadStrategy: "timeline",
    pagination: "local",
    streamPolicy: "none",
    supportsDisplayFilter: false,
    parameterEditor: "sql",
    capability: "localDatabase",
    configurable: true,
  }),
  yq: defineDescriptor({
    type: "yq",
    labelId: "timeline.yq",
    defaultName: "YQ",
    loadStrategy: "timeline",
    pagination: "local",
    streamPolicy: "none",
    supportsDisplayFilter: false,
    parameterEditor: "yq",
    capability: "localDatabase",
    configurable: true,
  }),
  search: defineDescriptor({
    type: "search",
    labelId: "timeline.search",
    defaultName: "Search",
    loadStrategy: "timeline",
    pagination: "local",
    streamPolicy: "none",
    supportsDisplayFilter: true,
    parameterEditor: "internal",
    capability: "dynamicOnly",
    configurable: false,
  }),
  user_bookmarks: defineDescriptor({
    type: "user_bookmarks",
    labelId: "timeline.userBookmarks",
    defaultName: "Bookmarks",
    loadStrategy: "timeline",
    pagination: "local",
    streamPolicy: "none",
    supportsDisplayFilter: false,
    parameterEditor: "internal",
    capability: "dynamicOnly",
    configurable: false,
  }),
  thread: defineDescriptor({
    type: "thread",
    labelId: "timeline.thread",
    defaultName: "Thread",
    loadStrategy: "thread",
    pagination: "none",
    streamPolicy: "none",
    supportsDisplayFilter: false,
    parameterEditor: "internal",
    capability: "dynamicOnly",
    configurable: false,
  }),
  profile: defineDescriptor({
    type: "profile",
    labelId: "timeline.profile",
    defaultName: "Profile",
    loadStrategy: "profile",
    pagination: "none",
    streamPolicy: "none",
    supportsDisplayFilter: false,
    parameterEditor: "internal",
    capability: "dynamicOnly",
    configurable: false,
  }),
  airContext: defineDescriptor({
    type: "airContext",
    labelId: "timeline.airContext",
    defaultName: "AIR context",
    loadStrategy: "airContext",
    pagination: "none",
    streamPolicy: "none",
    supportsDisplayFilter: false,
    parameterEditor: "internal",
    capability: "dynamicOnly",
    configurable: false,
  }),
} as const satisfies {
  [Type in KnownTimelineType]: TimelineDescriptor<Type>;
};

export function isKnownTimelineType(value: string): value is KnownTimelineType {
  return Object.prototype.hasOwnProperty.call(timelineDescriptorRegistry, value);
}

export function timelineDescriptor(
  value: string,
): TimelineDescriptor | undefined {
  return isKnownTimelineType(value)
    ? (timelineDescriptorRegistry[value] as TimelineDescriptor)
    : undefined;
}

export function timelineTypeIsAvailable(
  value: string,
  capabilities?: SessionCapabilities | null,
): boolean {
  const descriptor = timelineDescriptor(value);
  if (!descriptor || !descriptor.configurable) return false;
  if (
    descriptor.capability === "localDatabase" ||
    descriptor.capability === "dynamicOnly"
  ) {
    return true;
  }
  return capabilities ? capabilities.timelines[descriptor.capability] : true;
}

export function availableConfigurableTimelineTypes(
  capabilities?: SessionCapabilities | null,
): ConfigurableTimelineType[] {
  return configurableTimelineTypes.filter((type) =>
    timelineTypeIsAvailable(type, capabilities),
  );
}

/**
 * Unified Timeline availability is the union of every signed-in session.
 * The active account is a mutation actor and must not narrow timeline types.
 */
export function availableConfigurableTimelineTypesForSessions(
  capabilities: readonly SessionCapabilities[],
): ConfigurableTimelineType[] {
  return configurableTimelineTypes.filter((type) => {
    const descriptor = timelineDescriptorRegistry[type];
    if (descriptor.capability === "localDatabase") return true;
    return capabilities.some((session) =>
      timelineTypeIsAvailable(type, session),
    );
  });
}
