import React from "react";
import { createPortal } from "react-dom";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import {
  AlertTriangle,
  AtSign,
  BarChart3,
  Check,
  ChevronDown,
  ChevronLeft,
  ChevronRight,
  GripVertical,
  Hash,
  Loader2,
  Menu,
  Paperclip,
  Send,
  Smile,
  X,
} from "lucide-react";
import { invokeCommand, hasTauriRuntime } from "../../api/tauri";
import {
  unicodeEmojiCategories,
  pollDurations,
  pollDurationLabel,
  type UnicodeEmojiItem,
} from "../../constants/compose";
import { useAppStore, type AppStore } from "../../store/appStore";
import { appLocale, t } from "../../i18n";
import type {
  AccountSummary,
  ComposeMediaAttachment,
  CustomEmojiSummary,
  HashtagSuggestion,
  MediaAttachment,
  MentionSuggestion,
  TimelineStatus,
} from "../../types/app";
import { getClientPlatform } from "../../utils/browser";
import {
  fileToByteArray,
  filenameFromPath,
  statusPlainText,
} from "../../utils/format";
import { uniqueMediaSources } from "../../utils/media";
import { useRetriedMediaSource } from "../../utils/useRetriedMediaSource";
import { matchPresetVisibility } from "../../utils/visibility";
import { Avatar } from "../common/Avatar";
import { RetriedCustomEmojiImage } from "../common/CustomEmoji";
import { PostMenuPopover } from "../common/PostMenuPopover";

const visibilityOptions: Array<{
  value: AppStore["visibility"];
  label: string;
}> = [
  { value: "public", label: "Public" },
  { value: "unlisted", label: "Unlisted" },
  { value: "private", label: "Private" },
  { value: "direct", label: "Direct" },
];

const mimeExtensionMap: Record<string, string> = {
  "image/gif": "gif",
  "image/jpeg": "jpg",
  "image/png": "png",
  "image/webp": "webp",
};

const pastedImageFilename = (file: File, index: number) => {
  if (file.name.trim()) return file.name;
  const extension = mimeExtensionMap[file.type] ?? "png";
  return `pasted-image-${Date.now()}-${index + 1}.${extension}`;
};

type GraphemeSegmenter = {
  segment(input: string): Iterable<unknown>;
};

const graphemeSegmenter = (() => {
  const Segmenter = (
    Intl as unknown as {
      Segmenter?: new (
        locales?: string | string[],
        options?: { granularity: "grapheme" },
      ) => GraphemeSegmenter;
    }
  ).Segmenter;
  return Segmenter
    ? new Segmenter(undefined, { granularity: "grapheme" })
    : null;
})();

const countGraphemes = (value: string) =>
  graphemeSegmenter
    ? Array.from(graphemeSegmenter.segment(value)).length
    : Array.from(value).length;

const pollDurationDisplayLabel = (seconds: number) => {
  const duration = pollDurations.find((item) => item.seconds === seconds);
  if (!duration) return pollDurationLabel(seconds);
  return appLocale === "ja" ? duration.labelJa : duration.label;
};

type ComposeAutocompleteKind = "mention" | "hashtag" | "emoji";

type ComposeAutocompleteMatch = {
  kind: ComposeAutocompleteKind;
  query: string;
  start: number;
  end: number;
};

type ComposeAutocompleteItem = {
  value: string;
  label: string;
  insertText?: string;
  description?: string;
  avatar?: string;
  emoji?: CustomEmojiSummary;
  unicodeEmoji?: UnicodeEmojiItem;
};

type ComposeAutocompleteState = ComposeAutocompleteMatch & {
  items: ComposeAutocompleteItem[];
  selectedIndex: number;
  loading: boolean;
};

const autocompleteBoundaryChars = new Set([
  " ",
  "\n",
  "\t",
  "(",
  ")",
  "[",
  "]",
  "{",
  "}",
  "<",
  ">",
  '"',
  "'",
  "`",
]);

const detectComposeAutocomplete = (
  text: string,
  caret: number,
): ComposeAutocompleteMatch | null => {
  let start = caret;
  while (
    start > 0 &&
    !autocompleteBoundaryChars.has(text[start - 1] ?? "")
  ) {
    start -= 1;
  }
  const token = text.slice(start, caret);
  const emojiMarkerIndex = token.lastIndexOf(":");
  if (emojiMarkerIndex >= 0) {
    if (token.slice(0, emojiMarkerIndex).includes(":")) return null;
    const query = token.slice(emojiMarkerIndex + 1);
    if (query.length > 80 || !/^[\w+-]*$/.test(query)) return null;
    return {
      kind: "emoji",
      query,
      start: start + emojiMarkerIndex,
      end: caret,
    };
  }
  if (token.length < 2) return null;
  const marker = token[0];
  if (marker !== "@" && marker !== "#") return null;
  const query = token.slice(1).trim();
  if (!query || query.length > 80) return null;
  return {
    kind: marker === "@" ? "mention" : "hashtag",
    query,
    start,
    end: caret,
  };
};

const uniqueAutocompleteItems = (
  kind: ComposeAutocompleteKind,
  items: ComposeAutocompleteItem[],
) => {
  const seen = new Set<string>();
  return items.filter((item) => {
    const key = item.value
      .trim()
      .replace(
        kind === "mention" ? /^@/ : kind === "hashtag" ? /^#/ : /^:|:$/g,
        "",
      )
      .toLowerCase();
    if (!key || seen.has(key)) return false;
    seen.add(key);
    return true;
  });
};

const emojiAutocompleteItems = (
  emojis: CustomEmojiSummary[],
  query: string,
) => {
  const normalizedQuery = normalizeEmojiSearchText(query);
  const normalizedShortcodeQuery = query.toLowerCase();
  const customItems = emojis
    .filter((emoji) => {
      if (!normalizedQuery) return true;
      return normalizeEmojiSearchText(emoji.shortcode).includes(
        normalizedQuery,
      );
    })
    .sort((a, b) => {
      if (!normalizedQuery) return a.shortcode.localeCompare(b.shortcode);
      const aShortcode = a.shortcode.toLowerCase();
      const bShortcode = b.shortcode.toLowerCase();
      const aStarts = aShortcode.startsWith(normalizedShortcodeQuery);
      const bStarts = bShortcode.startsWith(normalizedShortcodeQuery);
      if (aStarts !== bStarts) return aStarts ? -1 : 1;
      return a.shortcode.localeCompare(b.shortcode);
    })
    .map((emoji) => ({
      value: emoji.shortcode,
      label: `:${emoji.shortcode}:`,
      insertText: `:${emoji.shortcode}:`,
      description: emoji.category ?? undefined,
      emoji,
    }));
  const unicodeItems = unicodeEmojiCategories
    .flatMap((category) => category.emojis)
    .filter((emoji) => {
      if (!normalizedQuery) return true;
      return emoji.searchText.includes(normalizedQuery);
    })
    .sort((a, b) => {
      if (!normalizedQuery) return a.name.localeCompare(b.name);
      const aName = normalizeEmojiSearchText(a.name);
      const bName = normalizeEmojiSearchText(b.name);
      const aStarts = aName.startsWith(normalizedQuery);
      const bStarts = bName.startsWith(normalizedQuery);
      if (aStarts !== bStarts) return aStarts ? -1 : 1;
      return a.name.localeCompare(b.name);
    })
    .map((emoji) => ({
      value: emoji.emoji,
      label: emoji.emoji,
      insertText: emoji.emoji,
      description: emoji.name,
      unicodeEmoji: emoji,
    }));
  const customLimit = unicodeItems.length > 0 ? 4 : 8;
  const mergedItems = [
    ...customItems.slice(0, customLimit),
    ...unicodeItems.slice(0, 8 - Math.min(customItems.length, customLimit)),
  ];
  if (mergedItems.length < 8) {
    mergedItems.push(...customItems.slice(customLimit, 8));
  }
  return uniqueAutocompleteItems("emoji", mergedItems).slice(0, 8);
};

export function ComposeArea() {
  const snapshot = useAppStore((state) => state.snapshot);
  const composeText = useAppStore((state) => state.composeText);
  const composeTarget = useAppStore((state) => state.composeTarget);
  const visibility = useAppStore((state) => state.visibility);
  const post = useAppStore((state) => state.post);
  const clearComposeTarget = useAppStore((state) => state.clearComposeTarget);
  const addBookmarksPane = useAppStore((state) => state.addBookmarksPane);
  const sectionRef = React.useRef<HTMLElement | null>(null);
  const textareaRef = React.useRef<HTMLTextAreaElement | null>(null);
  const fileInputRef = React.useRef<HTMLInputElement | null>(null);
  const dragIndexRef = React.useRef<number | null>(null);
  const [menuPosition, setMenuPosition] = React.useState<{
    top: number;
    left: number;
  } | null>(null);
  const [attachments, setAttachments] = React.useState<
    ComposeMediaAttachment[]
  >([]);
  const [dragIndex, setDragIndex] = React.useState<number | null>(null);
  const [cwEnabled, setCwEnabled] = React.useState(false);
  const [spoilerText, setSpoilerText] = React.useState("");
  const [pollEnabled, setPollEnabled] = React.useState(false);
  const [pollOptions, setPollOptions] = React.useState(["", ""]);
  const [pollMultiple, setPollMultiple] = React.useState(false);
  const [pollExpiresIn, setPollExpiresIn] = React.useState(24 * 60 * 60);
  const [emojiOpen, setEmojiOpen] = React.useState(false);
  const [customEmojis, setCustomEmojis] = React.useState<CustomEmojiSummary[]>(
    [],
  );
  const [customEmojisLoaded, setCustomEmojisLoaded] = React.useState(false);
  const [autocomplete, setAutocomplete] =
    React.useState<ComposeAutocompleteState | null>(null);
  const autocompleteRequestId = React.useRef(0);
  const customEmojiRequestRef =
    React.useRef<Promise<CustomEmojiSummary[]> | null>(null);
  const active =
    snapshot?.accounts.find(
      (account) => account.acct === snapshot.activeAcct,
    ) ?? snapshot?.accounts[0];
  const characterLimit = active?.characterLimit ?? 500;
  const characterCount = countGraphemes(composeText);
  const autoVisibility = React.useMemo(
    () =>
      matchPresetVisibility(snapshot?.settings.presetVisibility, composeText),
    [snapshot?.settings.presetVisibility, composeText],
  );
  const displayedVisibility = autoVisibility ?? visibility;
  const isMac = getClientPlatform() === "macos";
  const postShortcutLabel = isMac ? "Cmd+Enter" : "Ctrl+Enter";
  const uploading = attachments.some((attachment) => attachment.uploading);
  const validPollOptions = pollOptions
    .map((option) => option.trim())
    .filter(Boolean);
  const validPoll = pollEnabled && validPollOptions.length >= 2;
  const canPost =
    characterCount <= characterLimit &&
    !uploading &&
    (!pollEnabled || validPoll) &&
    (composeText.trim().length > 0 || attachments.length > 0 || validPoll);
  const composeHeight =
    112 +
    (composeTarget ? 28 : 0) +
    (cwEnabled ? 36 : 0) +
    (attachments.length > 0 ? 72 : 0) +
    (pollEnabled ? 112 : 0);
  const toggleMenu = (event: React.MouseEvent<HTMLButtonElement>) => {
    const rect = event.currentTarget.getBoundingClientRect();
    setMenuPosition((current) =>
      current
        ? null
        : {
            top: Math.min(rect.bottom + 4, window.innerHeight - 92),
            left: Math.max(8, rect.left),
          },
    );
  };
  const uploadFiles = async (files: File[]) => {
    const uploadableFiles = files.slice(0, Math.max(0, 4 - attachments.length));
    for (const file of uploadableFiles) {
      const localId =
        typeof crypto !== "undefined" && "randomUUID" in crypto
          ? crypto.randomUUID()
          : `${Date.now()}-${Math.random()}`;
      const previewSrc = URL.createObjectURL(file);
      setAttachments((current) =>
        [
          ...current,
          {
            id: localId,
            filename: file.name,
            previewSrc,
            uploading: true,
            media_type: file.type.startsWith("video/") ? "video" : "image",
          },
        ].slice(0, 4),
      );
      try {
        const uploaded = await invokeCommand<MediaAttachment>(
          "upload_compose_media",
          {
            request: {
              filename: file.name,
              mimeType: file.type || "application/octet-stream",
              data: await fileToByteArray(file),
            },
          },
        );
        setAttachments((current) =>
          current.map((attachment) =>
            attachment.id === localId
              ? {
                  ...attachment,
                  ...uploaded,
                  id: uploaded.id,
                  filename: file.name,
                  previewSrc:
                    uploaded.preview_url ??
                    uploaded.url ??
                    uploaded.remote_url ??
                    previewSrc,
                  uploading: false,
                }
              : attachment,
          ),
        );
      } catch (error) {
        URL.revokeObjectURL(previewSrc);
        setAttachments((current) =>
          current.filter((attachment) => attachment.id !== localId),
        );
        useAppStore.setState({ error: String(error) });
      }
    }
  };
  const handleComposePaste = (
    event: React.ClipboardEvent<HTMLTextAreaElement>,
  ) => {
    const pastedImages = Array.from(event.clipboardData.items)
      .filter((item) => item.kind === "file" && item.type.startsWith("image/"))
      .map((item, index) => {
        const file = item.getAsFile();
        if (!file) return null;
        return file.name.trim()
          ? file
          : new File([file], pastedImageFilename(file, index), {
              type: file.type,
              lastModified: file.lastModified,
            });
      })
      .filter((file): file is File => Boolean(file));
    if (!pastedImages.length) return;

    event.preventDefault();
    void uploadFiles(pastedImages);
  };
  const uploadDroppedPaths = async (paths: string[]) => {
    for (const path of paths.slice(0, Math.max(0, 4 - attachments.length))) {
      const filename = filenameFromPath(path);
      const localId =
        typeof crypto !== "undefined" && "randomUUID" in crypto
          ? crypto.randomUUID()
          : `${Date.now()}-${Math.random()}`;
      setAttachments((current) =>
        [
          ...current,
          {
            id: localId,
            filename,
            previewSrc: "",
            uploading: true,
            media_type: "unknown",
          },
        ].slice(0, 4),
      );
      try {
        const uploaded = await invokeCommand<MediaAttachment>(
          "upload_compose_media_path",
          { request: { path } },
        );
        setAttachments((current) =>
          current.map((attachment) =>
            attachment.id === localId
              ? {
                  ...attachment,
                  ...uploaded,
                  id: uploaded.id,
                  filename,
                  previewSrc:
                    uploaded.preview_url ??
                    uploaded.url ??
                    uploaded.remote_url ??
                    "",
                  uploading: false,
                }
              : attachment,
          ),
        );
      } catch (error) {
        setAttachments((current) =>
          current.filter((attachment) => attachment.id !== localId),
        );
        useAppStore.setState({ error: String(error) });
      }
    }
  };
  React.useEffect(() => {
    if (!hasTauriRuntime()) return;
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void getCurrentWebview()
      .onDragDropEvent((event) => {
        if (event.payload.type !== "drop") return;
        const rect = sectionRef.current?.getBoundingClientRect();
        if (rect && "position" in event.payload) {
          const x = event.payload.position.x / window.devicePixelRatio;
          const y = event.payload.position.y / window.devicePixelRatio;
          if (
            x < rect.left ||
            x > rect.right ||
            y < rect.top ||
            y > rect.bottom
          )
            return;
        }
        void uploadDroppedPaths(event.payload.paths);
      })
      .then((dispose) => {
        if (disposed) {
          dispose();
        } else {
          unlisten = dispose;
        }
      })
      .catch((error) => useAppStore.setState({ error: String(error) }));
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [attachments.length]);
  const removeAttachment = (index: number) => {
    setAttachments((current) => {
      const target = current[index];
      if (target?.previewSrc.startsWith("blob:"))
        URL.revokeObjectURL(target.previewSrc);
      return current.filter((_, itemIndex) => itemIndex !== index);
    });
  };
  const moveAttachment = (from: number, to: number) => {
    if (from === to) return;
    setAttachments((current) => {
      const next = [...current];
      const [item] = next.splice(from, 1);
      if (!item) return current;
      next.splice(to, 0, item);
      return next;
    });
  };
  const insertComposeText = (text: string) => {
    const textarea = textareaRef.current;
    const start = textarea?.selectionStart ?? composeText.length;
    const end = textarea?.selectionEnd ?? composeText.length;
    const next = `${composeText.slice(0, start)}${text}${composeText.slice(end)}`;
    useAppStore.setState({ composeText: next });
    requestAnimationFrame(() => {
      textarea?.focus();
      textarea?.setSelectionRange(start + text.length, start + text.length);
    });
  };
  const loadCustomEmojis = React.useCallback(() => {
    if (customEmojisLoaded) return Promise.resolve(customEmojis);
    if (!customEmojiRequestRef.current) {
      customEmojiRequestRef.current = invokeCommand<CustomEmojiSummary[]>(
        "custom_emojis",
      )
        .then((emojis) => {
          setCustomEmojis(emojis);
          setCustomEmojisLoaded(true);
          return emojis;
        })
        .catch((error) => {
          customEmojiRequestRef.current = null;
          throw error;
        });
    }
    return customEmojiRequestRef.current;
  }, [customEmojis, customEmojisLoaded]);
  const refreshAutocomplete = (text: string, caret: number) => {
    const match = detectComposeAutocomplete(text, caret);
    autocompleteRequestId.current += 1;
    const requestId = autocompleteRequestId.current;
    if (!match) {
      setAutocomplete(null);
      return;
    }
    setAutocomplete({
      ...match,
      items: [],
      selectedIndex: 0,
      loading: true,
    });
    if (match.kind === "emoji") {
      const updateEmojiSuggestions = (emojis: CustomEmojiSummary[]) => {
        if (autocompleteRequestId.current !== requestId) return;
        setAutocomplete({
          ...match,
          items: emojiAutocompleteItems(emojis, match.query),
          selectedIndex: 0,
          loading: false,
        });
      };
      if (customEmojisLoaded) {
        updateEmojiSuggestions(customEmojis);
        return;
      }
      setAutocomplete({
        ...match,
        items: emojiAutocompleteItems([], match.query),
        selectedIndex: 0,
        loading: false,
      });
      void loadCustomEmojis()
        .then(updateEmojiSuggestions)
        .catch((error) => {
          if (autocompleteRequestId.current !== requestId) return;
          console.debug("[awayuki][compose] emoji autocomplete failed", error);
        });
      return;
    }
    const command =
      match.kind === "mention"
        ? "autocomplete_mentions"
        : "autocomplete_hashtags";
    void invokeCommand<MentionSuggestion[] | HashtagSuggestion[]>(command, {
      request: {
        query: match.query,
        limit: 8,
        accountAcct: active?.acct ?? snapshot?.activeAcct ?? null,
      },
    })
      .then((suggestions) => {
        if (autocompleteRequestId.current !== requestId) return;
        const items = uniqueAutocompleteItems(
          match.kind,
          match.kind === "mention"
            ? (suggestions as MentionSuggestion[]).map((suggestion) => ({
                value: suggestion.acct,
                label: `@${suggestion.acct.replace(/^@/, "")}`,
                description: suggestion.displayName,
                avatar: suggestion.avatar,
              }))
            : (suggestions as HashtagSuggestion[]).map((suggestion) => ({
                value: suggestion.name,
                label: `#${suggestion.name.replace(/^#/, "")}`,
              })),
        );
        setAutocomplete({
          ...match,
          items,
          selectedIndex: 0,
          loading: false,
        });
      })
      .catch((error) => {
        if (autocompleteRequestId.current !== requestId) return;
        setAutocomplete(null);
        console.debug("[awayuki][compose] autocomplete failed", error);
      });
  };
  const applyAutocomplete = (item: ComposeAutocompleteItem) => {
    if (!autocomplete) return;
    const insertText =
      autocomplete.kind === "emoji"
        ? (item.insertText ?? `:${item.value.replace(/^:|:$/g, "")}:`)
        : `${autocomplete.kind === "mention" ? "@" : "#"}${item.value.replace(/^[@#]/, "")} `;
    const next = `${composeText.slice(0, autocomplete.start)}${insertText}${composeText.slice(autocomplete.end)}`;
    const caret = autocomplete.start + insertText.length;
    autocompleteRequestId.current += 1;
    useAppStore.setState({ composeText: next });
    setAutocomplete(null);
    requestAnimationFrame(() => {
      textareaRef.current?.focus();
      textareaRef.current?.setSelectionRange(caret, caret);
    });
  };
  const openEmojiPicker = () => {
    setEmojiOpen((current) => !current);
    void loadCustomEmojis().catch((error) =>
      useAppStore.setState({ error: String(error) }),
    );
  };
  const submit = async () => {
    const posted = await post({
      mediaIds: attachments
        .filter((attachment) => !attachment.uploading)
        .map((attachment) => attachment.id),
      spoilerText: cwEnabled ? spoilerText.trim() : undefined,
      sensitive: cwEnabled ? true : false,
      poll: validPoll
        ? {
            options: validPollOptions,
            multiple: pollMultiple,
            expiresIn: pollExpiresIn,
          }
        : undefined,
    });
    if (!posted) return;
    for (const attachment of attachments) {
      if (attachment.previewSrc.startsWith("blob:"))
        URL.revokeObjectURL(attachment.previewSrc);
    }
    setAttachments([]);
    setCwEnabled(false);
    setSpoilerText("");
    setPollEnabled(false);
    setPollOptions(["", ""]);
    setPollMultiple(false);
    setPollExpiresIn(24 * 60 * 60);
    setEmojiOpen(false);
  };
  const handleComposeKeyDown = (
    event: React.KeyboardEvent<HTMLTextAreaElement>,
  ) => {
    if (event.nativeEvent.isComposing) return;
    if (autocomplete) {
      if (event.key === "Escape") {
        event.preventDefault();
        autocompleteRequestId.current += 1;
        setAutocomplete(null);
        return;
      }
      if (event.key === "ArrowDown" || event.key === "ArrowUp") {
        event.preventDefault();
        setAutocomplete((current) => {
          if (!current || current.items.length === 0) return current;
          const direction = event.key === "ArrowDown" ? 1 : -1;
          return {
            ...current,
            selectedIndex:
              (current.selectedIndex + direction + current.items.length) %
              current.items.length,
          };
        });
        return;
      }
      if (
        (event.key === "Enter" || event.key === "Tab") &&
        autocomplete.items[autocomplete.selectedIndex]
      ) {
        const submitShortcut =
          event.key === "Enter" && (isMac ? event.metaKey : event.ctrlKey);
        if (!submitShortcut) {
          event.preventDefault();
          applyAutocomplete(autocomplete.items[autocomplete.selectedIndex]);
          return;
        }
      }
    }
    if (event.key !== "Enter") return;
    const submitShortcut = isMac ? event.metaKey : event.ctrlKey;
    if (!submitShortcut) return;

    event.preventDefault();
    if (canPost) void submit();
  };

  return (
    <section
      ref={sectionRef}
      className="grid shrink-0 grid-cols-[52px_minmax(0,1fr)] overflow-visible border-b border-surface0 bg-base"
      style={{ height: composeHeight }}
      onDragOver={(event) => {
        event.preventDefault();
        event.dataTransfer.dropEffect = "copy";
      }}
      onDrop={(event) => {
        event.preventDefault();
        const files = Array.from(event.dataTransfer.files);
        if (files.length) void uploadFiles(files);
      }}
    >
      <div className="flex min-h-0 flex-col px-2 py-2">
        <div className="flex flex-1 justify-center">
          <AccountQuickSwitcher
            accounts={snapshot?.accounts ?? []}
            activeAcct={active?.acct ?? snapshot?.activeAcct ?? null}
          />
        </div>
        <div className="flex h-8 items-end justify-start">
          <button
            className="btn btn-ghost btn-xs"
            onClick={toggleMenu}
            title={t("Menu")}
            data-post-menu-trigger
          >
            <Menu className="h-4 w-4" />
          </button>
          {menuPosition ? (
            <PostMenuPopover
              position={menuPosition}
              onClose={() => setMenuPosition(null)}
              items={[
                { label: t("Bookmarks"), action: addBookmarksPane },
                {
                  label: t("Settings"),
                  action: () =>
                    useAppStore.setState({
                      settingsOpen: true,
                      selectedSettings: "Account",
                    }),
                },
              ]}
            />
          ) : null}
        </div>
      </div>
      <div className="relative flex min-w-0 flex-col">
        <input
          ref={fileInputRef}
          className="hidden"
          type="file"
          multiple
          accept="image/*,video/*,audio/*"
          onChange={(event) => {
            const files = Array.from(event.currentTarget.files ?? []);
            event.currentTarget.value = "";
            if (files.length) void uploadFiles(files);
          }}
        />
        <div className="flex min-h-0 flex-1 flex-col px-2 pt-2">
          {cwEnabled ? (
            <input
              className="input input-bordered input-sm mb-1 h-8 min-h-8 w-full border-surface0 bg-base-100 text-sm"
              placeholder={t("Content warning")}
              value={spoilerText}
              onChange={(event) => setSpoilerText(event.target.value)}
            />
          ) : null}
          {composeTarget ? (
            <ComposeTargetPreview
              kind={composeTarget.kind}
              status={composeTarget.status}
              onClose={clearComposeTarget}
            />
          ) : null}
          <div className="relative flex min-h-[58px] flex-1">
            <textarea
              ref={textareaRef}
              id="compose-textarea"
              className="textarea h-full min-h-[58px] w-full resize-none border-surface0 bg-base-100 text-sm focus:border-blue focus:outline-none"
              placeholder={t("What's on your mind?")}
              value={composeText}
              onChange={(event) => {
                const next = event.target.value;
                useAppStore.setState({ composeText: next });
                refreshAutocomplete(next, event.target.selectionStart);
              }}
              onKeyDown={handleComposeKeyDown}
              onPaste={handleComposePaste}
              onSelect={(event) =>
                refreshAutocomplete(
                  event.currentTarget.value,
                  event.currentTarget.selectionStart,
                )
              }
            />
            {autocomplete ? (
              <ComposeAutocompletePopover
                kind={autocomplete.kind}
                items={autocomplete.items}
                loading={autocomplete.loading}
                selectedIndex={autocomplete.selectedIndex}
                onHover={(selectedIndex) =>
                  setAutocomplete((current) =>
                    current ? { ...current, selectedIndex } : current,
                  )
                }
                onSelect={applyAutocomplete}
              />
            ) : null}
          </div>
          {attachments.length ? (
            <div className="mt-1 flex h-16 items-center gap-1 overflow-x-auto">
              {attachments.map((attachment, index) => (
                <div
                  key={`${attachment.id}-${attachment.filename}`}
                  className={`group relative h-14 w-20 shrink-0 overflow-hidden rounded border bg-base-100 ${dragIndex !== null && dragIndex !== index ? "border-blue/70" : "border-surface0"}`}
                  onDragOver={(event) => {
                    event.preventDefault();
                    event.stopPropagation();
                    event.dataTransfer.dropEffect = "move";
                  }}
                  onDrop={(event) => {
                    event.preventDefault();
                    event.stopPropagation();
                    const fromText = event.dataTransfer.getData(
                      "application/x-awayuki-media-index",
                    );
                    const from =
                      dragIndexRef.current ??
                      (fromText ? Number(fromText) : Number.NaN);
                    if (Number.isFinite(from)) moveAttachment(from, index);
                    dragIndexRef.current = null;
                    setDragIndex(null);
                  }}
                  onDragEnd={() => {
                    dragIndexRef.current = null;
                    setDragIndex(null);
                  }}
                >
                  <ComposeAttachmentPreview attachment={attachment} />
                  <div
                    className="absolute left-0 top-0 grid h-5 w-5 cursor-grab place-items-center rounded-br bg-crust/80 text-subtext0 active:cursor-grabbing"
                    draggable
                    onDragStart={(event) => {
                      event.stopPropagation();
                      dragIndexRef.current = index;
                      setDragIndex(index);
                      event.dataTransfer.effectAllowed = "move";
                      event.dataTransfer.setData(
                        "application/x-awayuki-media-index",
                        String(index),
                      );
                    }}
                    title={t("Reorder media")}
                  >
                    <GripVertical className="h-3 w-3" />
                  </div>
                  <button
                    className="absolute right-0 top-0 grid h-5 w-5 place-items-center rounded-bl bg-crust/85 text-text"
                    onClick={() => removeAttachment(index)}
                    title={t("Remove media")}
                  >
                    <X className="h-3 w-3" />
                  </button>
                  {attachment.uploading ? (
                    <div className="absolute inset-0 grid place-items-center bg-crust/60">
                      <Loader2 className="h-4 w-4 animate-spin text-blue" />
                    </div>
                  ) : null}
                </div>
              ))}
            </div>
          ) : null}
          {pollEnabled ? (
            <ComposePollEditor
              options={pollOptions}
              multiple={pollMultiple}
              expiresIn={pollExpiresIn}
              onOptionsChange={setPollOptions}
              onMultipleChange={setPollMultiple}
              onExpiresInChange={setPollExpiresIn}
            />
          ) : null}
        </div>
        <div className="flex h-9 shrink-0 items-center justify-between gap-3 px-2">
          <div className="flex min-w-0 items-center gap-1">
            <button
              className="btn btn-ghost btn-xs"
              title={t("Attach media")}
              onClick={() => fileInputRef.current?.click()}
            >
              <Paperclip className="h-4 w-4" />
            </button>
            <button
              className={`btn btn-ghost btn-xs ${pollEnabled ? "bg-surface1 text-text" : ""}`}
              title={t("Poll")}
              onClick={() => setPollEnabled((current) => !current)}
            >
              <BarChart3 className="h-4 w-4" />
            </button>
            <button
              className={`btn btn-ghost btn-xs ${cwEnabled ? "bg-surface1 text-text" : ""}`}
              title={t("Content warning")}
              onClick={() => setCwEnabled((current) => !current)}
            >
              <AlertTriangle className="h-4 w-4" />
            </button>
            <button
              className={`btn btn-ghost btn-xs ${emojiOpen ? "bg-surface1 text-text" : ""}`}
              title={t("Emoji")}
              onClick={openEmojiPicker}
            >
              <Smile className="h-4 w-4" />
            </button>
            <VisibilityDropdown
              value={displayedVisibility}
              autoApplied={Boolean(autoVisibility)}
              onChange={(nextVisibility) =>
                useAppStore.setState({
                  visibility: nextVisibility,
                })
              }
            />
          </div>
          <div className="flex shrink-0 items-center gap-2">
            <span
              className={`text-xs ${characterCount > characterLimit ? "text-red" : "text-overlay0"}`}
            >
              {characterCount} / {characterLimit}
            </span>
            <button
              className="btn btn-primary btn-sm"
              onClick={() => void submit()}
              disabled={!canPost}
              title={t("Post ({shortcut})", { shortcut: postShortcutLabel })}
            >
              <Send className="h-4 w-4" />
              {t("Post")}
            </button>
          </div>
        </div>
        {emojiOpen ? (
          <ComposeEmojiPicker
            customEmojis={customEmojis}
            onPickEmoji={insertComposeText}
          />
        ) : null}
      </div>
    </section>
  );
}

function ComposeAutocompletePopover({
  kind,
  items,
  loading,
  selectedIndex,
  onHover,
  onSelect,
}: {
  kind: ComposeAutocompleteKind;
  items: ComposeAutocompleteItem[];
  loading: boolean;
  selectedIndex: number;
  onHover: (index: number) => void;
  onSelect: (item: ComposeAutocompleteItem) => void;
}) {
  if (!loading && items.length === 0) return null;
  return (
    <div className="absolute left-0 top-full z-30 mt-1 w-[min(360px,100%)] overflow-hidden rounded-md border border-surface0 bg-base-100 shadow-xl">
      {loading ? (
        <div className="flex h-9 items-center gap-2 px-3 text-xs text-subtext0">
          <Loader2 className="h-3.5 w-3.5 animate-spin text-blue" />
          {t("Loading")}
        </div>
      ) : (
        <div className="max-h-56 overflow-y-auto py-1">
          {items.map((item, index) => {
            const selected = index === selectedIndex;
            return (
              <button
                key={`${kind}-${item.value}-${index}`}
                type="button"
                className={`flex h-11 w-full items-center gap-2 px-2 text-left text-sm ${selected ? "bg-surface1 text-text" : "text-subtext0 hover:bg-surface0 hover:text-text"}`}
                onMouseEnter={() => onHover(index)}
                onMouseDown={(event) => {
                  event.preventDefault();
                  onSelect(item);
                }}
              >
                {item.emoji ? (
                  <span className="grid h-7 w-7 shrink-0 place-items-center rounded bg-surface0">
                    <RetriedCustomEmojiImage
                      emoji={item.emoji}
                      alt={item.label}
                      title={item.label}
                      className="max-h-5 max-w-5 object-contain"
                    />
                  </span>
                ) : item.unicodeEmoji ? (
                  <span className="grid h-7 w-7 shrink-0 place-items-center rounded bg-surface0 text-lg">
                    {item.unicodeEmoji.emoji}
                  </span>
                ) : item.avatar ? (
                  <Avatar
                    src={item.avatar}
                    label={item.description || item.value}
                    size="sm"
                  />
                ) : (
                  <span className="grid h-7 w-7 shrink-0 place-items-center rounded bg-surface0 text-blue">
                    {kind === "mention" ? (
                      <AtSign className="h-4 w-4" />
                    ) : kind === "hashtag" ? (
                      <Hash className="h-4 w-4" />
                    ) : (
                      <Smile className="h-4 w-4" />
                    )}
                  </span>
                )}
                <span className="min-w-0 flex-1">
                  <span className="block truncate font-medium text-text">
                    {item.label}
                  </span>
                  {item.description ? (
                    <span className="block truncate text-xs text-subtext0">
                      {item.description}
                    </span>
                  ) : null}
                </span>
              </button>
            );
          })}
        </div>
      )}
    </div>
  );
}

function ComposeTargetPreview({
  kind,
  status,
  onClose,
}: {
  kind: "reply" | "quote";
  status: TimelineStatus;
  onClose: () => void;
}) {
  const label = kind === "reply" ? t("Reply") : t("Quote");
  const previewText = statusPlainText(status) || `(${t("Media").toLowerCase()})`;

  return (
    <div className="mb-1 flex h-6 min-h-6 max-w-full items-center gap-2 overflow-hidden rounded border border-surface0 bg-base-100 px-2 text-xs text-subtext0">
      <span className="shrink-0 font-semibold text-text">{label}</span>
      <span className="shrink-0 text-overlay1">{status.acct}</span>
      <span className="min-w-0 flex-1 truncate">{previewText}</span>
      <button
        type="button"
        className="grid h-5 w-5 shrink-0 place-items-center rounded text-overlay0 hover:bg-surface0 hover:text-text"
        title={t("Clear target post")}
        aria-label={t("Clear target post")}
        onClick={onClose}
      >
        <X className="h-3.5 w-3.5" />
      </button>
    </div>
  );
}

function ComposeAttachmentPreview({
  attachment,
}: {
  attachment: ComposeMediaAttachment;
}) {
  const sources = React.useMemo(
    () =>
      uniqueMediaSources([
        attachment.previewSrc,
        attachment.preview_url,
        attachment.url,
        attachment.remote_url,
      ]),
    [
      attachment.previewSrc,
      attachment.preview_url,
      attachment.remote_url,
      attachment.url,
    ],
  );
  const image = useRetriedMediaSource(sources);

  return (
    <>
      {image.src && !image.failed ? (
        <img
          key={image.key}
          src={image.src}
          alt={attachment.filename}
          className={`h-full w-full object-cover ${image.loaded ? "" : "opacity-0"}`}
          draggable={false}
          onLoad={image.onLoad}
          onError={image.onError}
        />
      ) : null}
      {!image.loaded ? (
        <div className="absolute inset-0 grid place-items-center px-1 text-center text-[10px] text-subtext0">
          {image.retrying ? (
            <Loader2 className="h-4 w-4 animate-spin" />
          ) : (
            <span className="line-clamp-2 break-anywhere">
              {attachment.filename}
            </span>
          )}
        </div>
      ) : null}
    </>
  );
}

function AccountQuickSwitcher({
  accounts,
  activeAcct,
}: {
  accounts: AccountSummary[];
  activeAcct?: string | null;
}) {
  const switchAccount = useAppStore((state) => state.switchAccount);
  const [open, setOpen] = React.useState(false);
  const [position, setPosition] = React.useState<{
    top: number;
    left: number;
  } | null>(null);
  const [switchingAcct, setSwitchingAcct] = React.useState<string | null>(null);
  const buttonRef = React.useRef<HTMLButtonElement | null>(null);
  const active =
    accounts.find((account) => account.acct === activeAcct) ?? accounts[0];
  const canSwitch = accounts.length > 1;

  const updatePosition = React.useCallback(() => {
    const rect = buttonRef.current?.getBoundingClientRect();
    if (!rect) return;
    setPosition({
      top: Math.max(8, rect.top),
      left: Math.min(rect.right + 8, window.innerWidth - 312),
    });
  }, []);

  const openSwitcher = () => {
    if (!canSwitch) return;
    updatePosition();
    setOpen((current) => !current);
  };

  const chooseAccount = async (acct: string) => {
    if (acct === activeAcct) {
      setOpen(false);
      return;
    }
    setSwitchingAcct(acct);
    await switchAccount(acct);
    setSwitchingAcct(null);
    setOpen(false);
  };

  return (
    <>
      <button
        ref={buttonRef}
        type="button"
        className="group relative h-9 w-9 rounded-md focus:outline-none focus:ring-1 focus:ring-blue disabled:cursor-default"
        onClick={openSwitcher}
        disabled={!canSwitch}
        title={canSwitch ? t("Switch account") : (active?.acct ?? t("Account"))}
        aria-haspopup={canSwitch ? "menu" : undefined}
        aria-expanded={canSwitch ? open : undefined}
        data-account-switcher-trigger
      >
        <Avatar
          src={active?.avatar}
          label={active?.displayName || active?.acct || "A"}
          size="lg"
        />
        {canSwitch ? (
          <span className="absolute -bottom-0.5 -right-0.5 grid h-4 w-4 place-items-center rounded-full border border-base bg-surface0 text-subtext0 group-hover:text-text">
            <ChevronDown className="h-3 w-3" />
          </span>
        ) : null}
      </button>
      {open && position ? (
        <AccountSwitcherPopover
          accounts={accounts}
          activeAcct={activeAcct ?? active?.acct ?? null}
          switchingAcct={switchingAcct}
          position={position}
          onSelect={(acct) => void chooseAccount(acct)}
          onClose={() => setOpen(false)}
          onReposition={updatePosition}
        />
      ) : null}
    </>
  );
}

function AccountSwitcherPopover({
  accounts,
  activeAcct,
  switchingAcct,
  position,
  onSelect,
  onClose,
  onReposition,
}: {
  accounts: AccountSummary[];
  activeAcct?: string | null;
  switchingAcct: string | null;
  position: { top: number; left: number };
  onSelect: (acct: string) => void;
  onClose: () => void;
  onReposition: () => void;
}) {
  React.useEffect(() => {
    const close = (event: PointerEvent) => {
      const target = event.target as Element | null;
      if (
        target?.closest("[data-account-switcher]") ||
        target?.closest("[data-account-switcher-trigger]")
      ) {
        return;
      }
      onClose();
    };
    const closeOnKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") onClose();
    };
    const reposition = () => onReposition();

    document.addEventListener("pointerdown", close, true);
    document.addEventListener("keydown", closeOnKey);
    window.addEventListener("scroll", reposition, true);
    window.addEventListener("resize", reposition);
    return () => {
      document.removeEventListener("pointerdown", close, true);
      document.removeEventListener("keydown", closeOnKey);
      window.removeEventListener("scroll", reposition, true);
      window.removeEventListener("resize", reposition);
    };
  }, [onClose, onReposition]);

  return createPortal(
    <div
      className="fixed z-50 w-72 rounded-md border border-surface0 bg-base-100 p-1 text-sm text-text shadow-xl"
      style={{
        top: position.top,
        left: Math.max(8, position.left),
      }}
      data-account-switcher
      role="menu"
    >
      {accounts.map((account) => {
        const selected = account.acct === activeAcct;
        const switching = account.acct === switchingAcct;
        return (
          <button
            key={account.acct}
            type="button"
            className={`flex w-full items-center gap-2 rounded px-2 py-2 text-left hover:bg-surface0 disabled:cursor-wait disabled:hover:bg-transparent ${selected ? "bg-surface0 text-text" : "text-subtext0"}`}
            onClick={() => onSelect(account.acct)}
            disabled={switchingAcct !== null}
            role="menuitemradio"
            aria-checked={selected}
          >
            <span className="grid h-4 w-4 shrink-0 place-items-center text-blue">
              {selected ? <Check className="h-4 w-4" /> : null}
            </span>
            <Avatar
              src={account.avatar}
              label={account.displayName || account.acct}
              size="md"
            />
            <span className="min-w-0 flex-1">
              <span className="block truncate font-semibold text-text">
                {account.displayName || account.acct}
              </span>
              <span className="block truncate text-xs text-subtext0">
                @{account.acct}
              </span>
            </span>
            {switching ? (
              <Loader2 className="h-4 w-4 shrink-0 animate-spin text-blue" />
            ) : null}
          </button>
        );
      })}
    </div>,
    document.body,
  );
}

function VisibilityDropdown({
  value,
  autoApplied = false,
  onChange,
}: {
  value: AppStore["visibility"];
  autoApplied?: boolean;
  onChange: (value: AppStore["visibility"]) => void;
}) {
  const [open, setOpen] = React.useState(false);
  const selected =
    visibilityOptions.find((option) => option.value === value) ??
    visibilityOptions[0];

  return (
    <div
      className={`dropdown dropdown-bottom ${open ? "dropdown-open" : "dropdown-close"}`}
      onBlur={(event) => {
        if (!event.currentTarget.contains(event.relatedTarget)) {
          setOpen(false);
        }
      }}
    >
      <button
        type="button"
        className={`btn btn-outline btn-xs min-w-20 justify-between bg-base-100 px-2 font-normal text-text hover:border-surface1 hover:bg-surface0 hover:text-text ${autoApplied ? "border-blue" : "border-surface0"}`}
        title={autoApplied ? t("Auto visibility applied") : undefined}
        aria-haspopup="menu"
        aria-expanded={open}
        onClick={() => setOpen((current) => !current)}
      >
        {t(selected.label)}
        <ChevronDown className="h-3 w-3 text-subtext0" />
      </button>
      {open ? (
        <ul
          tabIndex={-1}
          className="dropdown-content menu z-50 w-36 rounded-box border border-surface0 bg-base-100 p-1 shadow"
          role="menu"
        >
          {visibilityOptions.map((option) => (
            <li key={option.value}>
              <button
                type="button"
                className={option.value === value ? "active" : ""}
                role="menuitemradio"
                aria-checked={option.value === value}
                onClick={() => {
                  onChange(option.value);
                  setOpen(false);
                }}
              >
                {t(option.label)}
              </button>
            </li>
          ))}
        </ul>
      ) : null}
    </div>
  );
}

function ComposePollEditor({
  options,
  multiple,
  expiresIn,
  onOptionsChange,
  onMultipleChange,
  onExpiresInChange,
}: {
  options: string[];
  multiple: boolean;
  expiresIn: number;
  onOptionsChange: (options: string[]) => void;
  onMultipleChange: (multiple: boolean) => void;
  onExpiresInChange: (expiresIn: number) => void;
}) {
  return (
    <div className="mt-1 border-t border-surface0 bg-surface0/70 py-1 text-sm">
      <div className="space-y-1">
        {options.map((option, index) => (
          <div key={index} className="flex items-center gap-2 px-1">
            <span className="h-2.5 w-2.5 rounded-full border border-overlay0" />
            <input
              className="input input-bordered input-xs h-7 min-h-7 flex-1 border-surface1 bg-base-100"
              placeholder={t("Option {index}", { index: index + 1 })}
              value={option}
              onChange={(event) =>
                onOptionsChange(
                  options.map((item, itemIndex) =>
                    itemIndex === index ? event.target.value : item,
                  ),
                )
              }
            />
            {options.length > 2 ? (
              <button
                className="btn btn-ghost btn-xs"
                onClick={() =>
                  onOptionsChange(
                    options.filter((_, itemIndex) => itemIndex !== index),
                  )
                }
                title={t("Remove option")}
              >
                <X className="h-3.5 w-3.5" />
              </button>
            ) : null}
          </div>
        ))}
      </div>
      <div className="mt-1 flex items-center gap-2 px-1">
        <button
          className="btn btn-ghost btn-xs h-7 min-h-7 px-2 text-xs"
          onClick={() => onOptionsChange([...options, ""])}
          disabled={options.length >= 4}
        >
          {t("Add option")}
        </button>
        <div className="join">
          <button
            className={`btn join-item btn-xs h-7 min-h-7 border-blue bg-blue px-2 text-xs text-black hover:border-sapphire hover:bg-sapphire hover:text-black ${!multiple ? "btn-active" : ""}`}
            onClick={() => onMultipleChange(false)}
          >
            {t("Single")}
          </button>
          <button
            className={`btn join-item btn-xs h-7 min-h-7 border-blue bg-blue px-2 text-xs text-black hover:border-sapphire hover:bg-sapphire hover:text-black ${multiple ? "btn-active" : ""}`}
            onClick={() => onMultipleChange(true)}
          >
            {t("Multiple")}
          </button>
        </div>
        <PollDurationDropdown value={expiresIn} onChange={onExpiresInChange} />
        <span className="text-xs text-subtext0">
          {pollDurationDisplayLabel(expiresIn)}
        </span>
      </div>
    </div>
  );
}

function PollDurationDropdown({
  value,
  onChange,
}: {
  value: number;
  onChange: (value: number) => void;
}) {
  const [open, setOpen] = React.useState(false);
  const selected =
    pollDurations.find((duration) => duration.seconds === value) ??
    pollDurations[0];

  return (
    <div
      className={`dropdown dropdown-bottom ${open ? "dropdown-open" : "dropdown-close"}`}
      onBlur={(event) => {
        if (!event.currentTarget.contains(event.relatedTarget)) {
          setOpen(false);
        }
      }}
    >
      <button
        type="button"
        className="btn btn-outline btn-xs h-8 min-h-8 min-w-24 justify-between border-blue bg-base-100 px-2 font-normal text-text hover:border-sapphire hover:bg-surface0 hover:text-text"
        aria-haspopup="menu"
        aria-expanded={open}
        onClick={() => setOpen((current) => !current)}
      >
        {pollDurationDisplayLabel(selected.seconds)}
        <ChevronDown className="h-3 w-3 text-subtext0" />
      </button>
      {open ? (
        <ul
          tabIndex={-1}
          className="dropdown-content menu z-50 w-32 rounded-box border border-surface0 bg-base-100 p-1 shadow"
          role="menu"
        >
          {pollDurations.map((duration) => (
            <li key={duration.seconds}>
              <button
                type="button"
                className={duration.seconds === value ? "active" : ""}
                role="menuitemradio"
                aria-checked={duration.seconds === value}
                onClick={() => {
                  onChange(duration.seconds);
                  setOpen(false);
                }}
              >
                {pollDurationDisplayLabel(duration.seconds)}
              </button>
            </li>
          ))}
        </ul>
      ) : null}
    </div>
  );
}

function ComposeEmojiPicker({
  customEmojis,
  onPickEmoji,
}: {
  customEmojis: CustomEmojiSummary[];
  onPickEmoji: (emoji: string) => void;
}) {
  const [query, setQuery] = React.useState("");
  const normalizedQuery = normalizeEmojiSearchText(query);
  const customGroups = React.useMemo(() => {
    const map = new Map<string, CustomEmojiSummary[]>();
    for (const emoji of customEmojis) {
      const category = emoji.category?.trim() || "Custom";
      const items = map.get(category) ?? [];
      items.push(emoji);
      map.set(category, items);
    }
    return [...map.entries()].map(([name, emojis]) => ({
      name,
      icon: emojis[0]?.url,
      iconEmoji: emojis[0],
      emojis,
    }));
  }, [customEmojis]);
  const categories = [...unicodeEmojiCategories, ...customGroups];
  const [activeCategory, setActiveCategory] = React.useState(
    categories[0]?.name ?? "Smileys & Emotion",
  );
  const visibleCategoryCount = 6;
  const [categoryOffset, setCategoryOffset] = React.useState(0);
  React.useEffect(() => {
    const maxOffset = Math.max(0, categories.length - visibleCategoryCount);
    setCategoryOffset((offset) => Math.min(offset, maxOffset));
    if (!categories.some((category) => category.name === activeCategory)) {
      setActiveCategory(categories[0]?.name ?? "");
    }
  }, [activeCategory, categories.length]);
  const active =
    categories.find((category) => category.name === activeCategory) ??
    categories[0];
  const visibleCategories = categories.slice(
    categoryOffset,
    categoryOffset + visibleCategoryCount,
  );
  const visibleEmojis = React.useMemo(() => {
    if (normalizedQuery) {
      const unicodeEmojis = unicodeEmojiCategories.flatMap((category) =>
        category.emojis.filter((emoji) =>
          emoji.searchText.includes(normalizedQuery),
        ),
      );
      const customEmojiResults = customGroups.flatMap((category) =>
        category.emojis.filter((emoji) =>
          normalizeEmojiSearchText(
            [emoji.shortcode, emoji.category ?? ""].join(" "),
          ).includes(normalizedQuery),
        ),
      );
      return [...unicodeEmojis, ...customEmojiResults];
    }

    return active && "emojis" in active ? active.emojis : [];
  }, [active, customGroups, normalizedQuery]);
  const activeLabel = normalizedQuery
    ? t("Search results")
    : active?.name
      ? t(active.name)
      : "";

  return (
    <div className="absolute left-2 top-full z-40 mt-1 w-[365px] rounded-md border border-surface0 bg-base-100 p-3 text-sm text-text shadow-xl">
      <div className="mb-2 grid h-8 grid-cols-[28px_minmax(0,1fr)_28px] items-center gap-1">
        <button
          className="btn btn-ghost btn-xs h-7 min-h-7 px-1"
          onClick={() => setCategoryOffset((offset) => Math.max(0, offset - 1))}
          disabled={categoryOffset === 0}
          title={t("Previous emoji categories")}
        >
          <ChevronLeft className="h-4 w-4" />
        </button>
        <div className="flex min-w-0 items-center justify-center gap-2 overflow-hidden">
          {visibleCategories.map((category) => (
            <button
              key={category.name}
              className={`grid h-8 min-w-10 place-items-center rounded-full px-2 text-lg ${active?.name === category.name ? "bg-blue text-crust" : "hover:bg-surface0"}`}
              onClick={() => setActiveCategory(category.name)}
              title={t(category.name)}
            >
              {"iconEmoji" in category && category.iconEmoji ? (
                <RetriedCustomEmojiImage
                  emoji={category.iconEmoji}
                  alt=""
                  title={t(category.name)}
                  className="h-5 w-5 object-contain"
                />
              ) : (
                category.icon
              )}
            </button>
          ))}
        </div>
        <button
          className="btn btn-ghost btn-xs h-7 min-h-7 px-1"
          onClick={() =>
            setCategoryOffset((offset) =>
              Math.min(
                Math.max(0, categories.length - visibleCategoryCount),
                offset + 1,
              ),
            )
          }
          disabled={categoryOffset + visibleCategoryCount >= categories.length}
          title={t("Next emoji categories")}
        >
          <ChevronRight className="h-4 w-4" />
        </button>
      </div>
      <input
        className="input input-bordered input-sm mb-3 h-8 min-h-8 w-full border-surface0 bg-base-100 text-sm"
        placeholder={t("Search emoji...")}
        value={query}
        onChange={(event) => setQuery(event.target.value)}
      />
      <div className="mb-2 text-xs text-subtext0">
        {activeLabel}
      </div>
      <div className="grid max-h-60 grid-cols-9 gap-2 overflow-y-auto overflow-x-hidden pr-1">
        {visibleEmojis.map((emoji, index) =>
          isUnicodeEmojiItem(emoji) ? (
            <button
              key={`${emoji.codePointsHex.join("-")}-${index}`}
              className="grid h-7 w-7 place-items-center overflow-hidden rounded text-xl hover:bg-surface0"
              onClick={() => onPickEmoji(emoji.emoji)}
              title={emoji.name}
            >
              {emoji.emoji}
            </button>
          ) : (
            <button
              key={emoji.shortcode}
              className="grid h-7 w-7 place-items-center overflow-hidden rounded hover:bg-surface0"
              onClick={() => onPickEmoji(`:${emoji.shortcode}:`)}
              title={`:${emoji.shortcode}:`}
            >
              <RetriedCustomEmojiImage
                emoji={emoji}
                alt={emoji.shortcode}
                title={`:${emoji.shortcode}:`}
                className="max-h-6 max-w-6 object-contain"
              />
            </button>
          ),
        )}
      </div>
    </div>
  );
}

function isUnicodeEmojiItem(
  emoji: UnicodeEmojiItem | CustomEmojiSummary,
): emoji is UnicodeEmojiItem {
  return "emoji" in emoji;
}

function normalizeEmojiSearchText(value: string) {
  return value
    .toLowerCase()
    .replace(/[:_-]+/g, " ")
    .replace(/\s+/g, " ")
    .trim();
}
