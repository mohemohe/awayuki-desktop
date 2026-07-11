import React from "react";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import {
  AlertTriangle,
  BarChart3,
  Loader2,
  Menu,
  Paperclip,
  Send,
  Smile,
} from "lucide-react";
import {
  hasTauriRuntime,
  invokeReadCommand,
} from "../../api/tauri";
import {
  uploadBrowserFile,
  uploadDroppedMediaPath,
} from "../../api/mediaUpload";
import {
  detectComposeAutocomplete,
  emojiAutocompleteItems,
  uniqueAutocompleteItems,
  type ComposeAutocompleteItem,
  type ComposeAutocompleteState,
} from "../../domain/composeAutocomplete";
import type { UnicodeEmojiCategory } from "../../constants/unicodeEmoji";
import { useAppStore } from "../../store/appStore";
import { t } from "../../i18n";
import type {
  ComposeMediaAttachment,
  CustomEmojiSummary,
  HashtagSuggestion,
  MentionSuggestion,
} from "../../types/app";
import { getClientPlatform } from "../../utils/browser";
import { filenameFromPath } from "../../utils/format";
import {
  frontendRequestScheduler,
  RequestCancelledError,
} from "../../utils/requestScheduler";
import { matchPresetVisibility } from "../../utils/visibility";
import { PostMenuPopover } from "../common/PostMenuPopover";
import {
  LiveRegion,
} from "../primitives/Listbox";
import { moveQueueItem } from "../../features/compose/mediaQueue";
import { useAppLocale } from "../../hooks/useAppLocale";
import { ComposeAutocompleteListbox } from "../../features/compose/ComposeAutocompleteListbox";
import { AccountQuickSwitcher } from "../../features/compose/AccountQuickSwitcher";
import { ComposeAttachmentStrip } from "../../features/compose/ComposeAttachmentStrip";
import { ComposeEmojiPicker } from "../../features/compose/ComposeEmojiPicker";
import { ComposePollEditor } from "../../features/compose/ComposePollEditor";
import { ComposeTargetPreview } from "../../features/compose/ComposeTargetPreview";
import { VisibilityDropdown } from "../../features/compose/VisibilityDropdown";
import { ComposeAreaView } from "./ComposeAreaView";

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

export function ComposeAreaController() {
  useAppLocale();
  const snapshot = useAppStore((state) => state.snapshot);
  const composeText = useAppStore((state) => state.composeText);
  const composeTarget = useAppStore((state) => state.composeTarget);
  const visibility = useAppStore((state) => state.visibility);
  const post = useAppStore((state) => state.post);
  const postMutation = useAppStore(
    (state) => state.mutationStates["compose:submit"],
  );
  const clearComposeTarget = useAppStore((state) => state.clearComposeTarget);
  const addBookmarksPane = useAppStore((state) => state.addBookmarksPane);
  const addFavouritesPane = useAppStore((state) => state.addFavouritesPane);
  const sectionRef = React.useRef<HTMLElement | null>(null);
  const textareaRef = React.useRef<HTMLTextAreaElement | null>(null);
  const fileInputRef = React.useRef<HTMLInputElement | null>(null);
  const [menuPosition, setMenuPosition] = React.useState<{
    top: number;
    left: number;
  } | null>(null);
  const [attachments, setAttachments] = React.useState<
    ComposeMediaAttachment[]
  >([]);
  const [mediaAnnouncement, setMediaAnnouncement] = React.useState("");
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
  const [unicodeEmojiCategories, setUnicodeEmojiCategories] = React.useState<
    UnicodeEmojiCategory[]
  >([]);
  const [unicodeEmojisLoaded, setUnicodeEmojisLoaded] = React.useState(false);
  const [autocomplete, setAutocomplete] =
    React.useState<ComposeAutocompleteState | null>(null);
  const autocompleteRequestId = React.useRef(0);
  const autocompleteTimerRef = React.useRef<number | null>(null);
  const customEmojiRequestRef =
    React.useRef<Promise<CustomEmojiSummary[]> | null>(null);
  const unicodeEmojiRequestRef =
    React.useRef<Promise<UnicodeEmojiCategory[]> | null>(null);
  const active =
    snapshot?.accounts.find(
      (account) => account.acct === snapshot.activeAcct,
    ) ?? snapshot?.accounts[0];
  const activeAcct = active?.acct ?? null;
  const maxAttachments = active?.capabilities.compose.maxMediaAttachments ?? 0;
  const mediaUploadSupported = active?.capabilities.compose.mediaUpload ?? false;
  const pollSupported = active?.capabilities.compose.poll ?? false;
  const accountGenerationRef = React.useRef(1);
  const activeAcctRef = React.useRef<string | null>(activeAcct);
  const previousActiveAcctRef = React.useRef<string | null>(activeAcct);
  const uploadControllersRef = React.useRef(new Map<string, AbortController>());
  const attachmentsRef = React.useRef<ComposeMediaAttachment[]>([]);
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
  const isEditing = composeTarget?.kind === "edit";
  const uploading = attachments.some((attachment) => attachment.uploading);
  const posting = postMutation?.phase === "pending";
  const validPollOptions = pollOptions
    .map((option) => option.trim())
    .filter(Boolean);
  const validPoll = pollEnabled && validPollOptions.length >= 2;
  const hasComposeText = composeText.trim().length > 0;
  const canPost =
    characterCount <= characterLimit &&
    !uploading &&
    !posting &&
    (!pollEnabled || (pollSupported && validPoll)) &&
    (isEditing
      ? hasComposeText
      : hasComposeText || attachments.length > 0 || validPoll);
  const submitLabel = isEditing ? t("Edit post") : t("Post");
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
  React.useEffect(() => {
    attachmentsRef.current = attachments;
  }, [attachments]);
  React.useEffect(() => {
    activeAcctRef.current = activeAcct;
    if (previousActiveAcctRef.current === activeAcct) return;
    previousActiveAcctRef.current = activeAcct;
    accountGenerationRef.current += 1;
    for (const controller of uploadControllersRef.current.values()) {
      controller.abort();
    }
    uploadControllersRef.current.clear();
    setAttachments((current) => {
      for (const attachment of current) {
        if (attachment.previewSrc.startsWith("blob:")) {
          URL.revokeObjectURL(attachment.previewSrc);
        }
      }
      return [];
    });
    setCustomEmojis([]);
    setCustomEmojisLoaded(false);
    customEmojiRequestRef.current = null;
    setAutocomplete(null);
    useAppStore.setState({ composeText: "", composeTarget: null });
  }, [activeAcct]);
  React.useEffect(
    () => () => {
      accountGenerationRef.current += 1;
      for (const controller of uploadControllersRef.current.values()) {
        controller.abort();
      }
      uploadControllersRef.current.clear();
      for (const attachment of attachmentsRef.current) {
        if (attachment.previewSrc.startsWith("blob:")) {
          URL.revokeObjectURL(attachment.previewSrc);
        }
      }
    },
    [],
  );
  const uploadFiles = async (files: File[]) => {
    if (isEditing || !activeAcct || !mediaUploadSupported) return;
    const actingAccountAcct = activeAcct;
    const generation = accountGenerationRef.current;
    const uploadableFiles = files.slice(
      0,
      Math.max(0, maxAttachments - attachments.length),
    );
    for (const file of uploadableFiles) {
      const localId =
        typeof crypto !== "undefined" && "randomUUID" in crypto
          ? crypto.randomUUID()
          : `${Date.now()}-${Math.random()}`;
      const previewSrc = URL.createObjectURL(file);
      const controller = new AbortController();
      uploadControllersRef.current.set(localId, controller);
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
        ].slice(0, maxAttachments),
      );
      try {
        const uploaded = await uploadBrowserFile(actingAccountAcct, file, {
          signal: controller.signal,
          onProgress: ({ written, total }) => {
            if (
              generation !== accountGenerationRef.current ||
              activeAcctRef.current !== actingAccountAcct
            )
              return;
            setAttachments((current) =>
              current.map((attachment) =>
                attachment.id === localId
                  ? {
                      ...attachment,
                      uploadProgress: total > 0 ? written / total : 0,
                    }
                  : attachment,
              ),
            );
          },
        });
        if (
          generation !== accountGenerationRef.current ||
          activeAcctRef.current !== actingAccountAcct
        ) {
          URL.revokeObjectURL(previewSrc);
          continue;
        }
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
                  uploadProgress: 1,
                }
              : attachment,
          ),
        );
      } catch (error) {
        URL.revokeObjectURL(previewSrc);
        setAttachments((current) =>
          current.filter((attachment) => attachment.id !== localId),
        );
        if (!controller.signal.aborted) {
          useAppStore.setState({ error: String(error) });
        }
      } finally {
        uploadControllersRef.current.delete(localId);
      }
    }
  };
  const handleComposePaste = (
    event: React.ClipboardEvent<HTMLTextAreaElement>,
  ) => {
    if (isEditing) return;
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
  const uploadDroppedPaths = React.useCallback(async (paths: string[]) => {
    if (isEditing || !activeAcct || !mediaUploadSupported) return;
    const actingAccountAcct = activeAcct;
    const generation = accountGenerationRef.current;
    for (const path of paths.slice(
      0,
      Math.max(0, maxAttachments - attachments.length),
    )) {
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
        ].slice(0, maxAttachments),
      );
      try {
        const uploaded = await uploadDroppedMediaPath(actingAccountAcct, path);
        if (
          generation !== accountGenerationRef.current ||
          activeAcctRef.current !== actingAccountAcct
        )
          continue;
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
        if (
          generation === accountGenerationRef.current &&
          activeAcctRef.current === actingAccountAcct
        ) {
          useAppStore.setState({ error: String(error) });
        }
      }
    }
  }, [activeAcct, attachments.length, isEditing, maxAttachments, mediaUploadSupported]);
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
  }, [uploadDroppedPaths]);
  const removeAttachment = (index: number) => {
    setAttachments((current) => {
      const target = current[index];
      if (target?.previewSrc.startsWith("blob:"))
        URL.revokeObjectURL(target.previewSrc);
      return current.filter((_, itemIndex) => itemIndex !== index);
    });
  };
  const moveAttachment = (from: number, to: number, announce = false) => {
    if (from === to) return;
    setAttachments((current) => moveQueueItem(current, from, to));
    if (announce) {
      setMediaAnnouncement(t("a11y.media.moved", { position: to + 1 }));
    }
  };
  const editTargetKeyRef = React.useRef<string | null>(null);
  React.useEffect(() => {
    if (composeTarget?.kind !== "edit") {
      editTargetKeyRef.current = null;
      return;
    }
    const editTargetKey = `${composeTarget.status.serverDomain}:${composeTarget.status.originalStatusId}`;
    if (editTargetKeyRef.current === editTargetKey) return;
    editTargetKeyRef.current = editTargetKey;
    setAttachments((current) => {
      for (const attachment of current) {
        if (attachment.previewSrc.startsWith("blob:")) {
          URL.revokeObjectURL(attachment.previewSrc);
        }
      }
      return [];
    });
    setCwEnabled(Boolean(composeTarget.status.spoilerText));
    setSpoilerText(composeTarget.status.spoilerText ?? "");
    setPollEnabled(false);
    setPollOptions(["", ""]);
    setPollMultiple(false);
    setPollExpiresIn(24 * 60 * 60);
    setEmojiOpen(false);
    setAutocomplete(null);
  }, [composeTarget]);
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
    if (!activeAcct) return Promise.resolve([]);
    if (customEmojisLoaded) return Promise.resolve(customEmojis);
    if (!customEmojiRequestRef.current) {
      const generation = accountGenerationRef.current;
      const actingAccountAcct = activeAcct;
      customEmojiRequestRef.current = invokeReadCommand<CustomEmojiSummary[]>(
        "custom_emojis",
        { accountAcct: actingAccountAcct },
      )
        .then((emojis) => {
          if (
            generation !== accountGenerationRef.current ||
            activeAcctRef.current !== actingAccountAcct
          ) {
            return [];
          }
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
  }, [activeAcct, customEmojis, customEmojisLoaded]);
  const loadUnicodeEmojis = React.useCallback(() => {
    if (unicodeEmojisLoaded) return Promise.resolve(unicodeEmojiCategories);
    if (!unicodeEmojiRequestRef.current) {
      unicodeEmojiRequestRef.current = import("../../constants/unicodeEmoji")
        .then((module) => {
          setUnicodeEmojiCategories(module.unicodeEmojiCategories);
          setUnicodeEmojisLoaded(true);
          return module.unicodeEmojiCategories;
        })
        .catch((error) => {
          unicodeEmojiRequestRef.current = null;
          throw error;
        });
    }
    return unicodeEmojiRequestRef.current;
  }, [unicodeEmojiCategories, unicodeEmojisLoaded]);
  const refreshAutocomplete = (text: string, caret: number) => {
    const match = detectComposeAutocomplete(text, caret);
    autocompleteRequestId.current += 1;
    const requestId = autocompleteRequestId.current;
    if (autocompleteTimerRef.current !== null) {
      window.clearTimeout(autocompleteTimerRef.current);
      autocompleteTimerRef.current = null;
    }
    frontendRequestScheduler.cancel("autocomplete:compose");
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
      const updateEmojiSuggestions = (
        emojis: CustomEmojiSummary[],
        categories: UnicodeEmojiCategory[],
        loading: boolean,
      ) => {
        if (autocompleteRequestId.current !== requestId) return;
        setAutocomplete({
          ...match,
          items: emojiAutocompleteItems(emojis, categories, match.query),
          selectedIndex: 0,
          loading,
        });
      };
      const initialCustomEmojis = customEmojisLoaded ? customEmojis : [];
      const initialUnicodeCategories = unicodeEmojisLoaded
        ? unicodeEmojiCategories
        : [];
      const loading = !customEmojisLoaded || !unicodeEmojisLoaded;
      updateEmojiSuggestions(
        initialCustomEmojis,
        initialUnicodeCategories,
        loading,
      );
      if (!loading) return;
      void Promise.allSettled([loadCustomEmojis(), loadUnicodeEmojis()])
        .then(([customResult, unicodeResult]) => {
          const nextCustomEmojis =
            customResult.status === "fulfilled"
              ? customResult.value
              : initialCustomEmojis;
          const nextUnicodeCategories =
            unicodeResult.status === "fulfilled"
              ? unicodeResult.value
              : initialUnicodeCategories;
          updateEmojiSuggestions(nextCustomEmojis, nextUnicodeCategories, false);
        })
        .catch((error) => {
          if (autocompleteRequestId.current !== requestId) return;
          console.debug("[awayuki][compose] emoji autocomplete failed", error);
        });
      return;
    }
    if (match.query.length < 2) {
      setAutocomplete(null);
      return;
    }
    const command =
      match.kind === "mention"
        ? "autocomplete_mentions"
        : "autocomplete_hashtags";
    autocompleteTimerRef.current = window.setTimeout(() => {
      autocompleteTimerRef.current = null;
      let resourceGeneration = 0;
      void frontendRequestScheduler
        .schedule<MentionSuggestion[] | HashtagSuggestion[]>(
          {
            key: "autocomplete:compose",
            lane: "autocomplete",
            priority: 100,
          },
          async (context) => {
            resourceGeneration = context.generation;
            useAppStore.setState((state) => ({
              resourceStates: {
                ...state.resourceStates,
                "autocomplete:compose": {
                  generation: context.generation,
                  phase: "loading",
                },
              },
            }));
            const suggestions = await invokeReadCommand<
              MentionSuggestion[] | HashtagSuggestion[]
            >(command, {
              request: {
                query: match.query,
                limit: 8,
                accountAcct: active?.acct ?? snapshot?.activeAcct ?? null,
              },
            });
            if (!context.isCurrent()) {
              throw new RequestCancelledError("autocomplete:compose");
            }
            useAppStore.setState((state) =>
              state.resourceStates["autocomplete:compose"]?.generation ===
              context.generation
                ? {
                    resourceStates: {
                      ...state.resourceStates,
                      "autocomplete:compose": {
                        generation: context.generation,
                        phase: "succeeded",
                      },
                    },
                  }
                : {},
            );
            return suggestions.slice(0, 8);
          },
        )
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
        const cancelled = error instanceof RequestCancelledError;
        useAppStore.setState((state) =>
          state.resourceStates["autocomplete:compose"]?.generation ===
          resourceGeneration
            ? {
                resourceStates: {
                  ...state.resourceStates,
                  "autocomplete:compose": {
                    generation: resourceGeneration,
                    phase: cancelled ? "cancelled" : "failed",
                    ...(cancelled ? {} : { error: String(error) }),
                  },
                },
              }
            : {},
        );
        if (autocompleteRequestId.current !== requestId) return;
        if (cancelled) return;
        setAutocomplete(null);
        console.debug("[awayuki][compose] autocomplete failed", error);
      })
      .finally(() => {
        useAppStore.setState({
          requestMetrics: frontendRequestScheduler.metrics(),
        });
      });
    }, 250);
  };

  React.useEffect(() => {
    return () => {
      autocompleteRequestId.current += 1;
      if (autocompleteTimerRef.current !== null) {
        window.clearTimeout(autocompleteTimerRef.current);
      }
      frontendRequestScheduler.cancel("autocomplete:compose");
    };
  }, [active?.acct]);
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
    void Promise.all([loadCustomEmojis(), loadUnicodeEmojis()]).catch(
      (error) => useAppStore.setState({ error: String(error) }),
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
    <ComposeAreaView
      sectionRef={sectionRef}
      height={composeHeight}
      mediaDropEnabled={!isEditing}
      onDropFiles={(files) => void uploadFiles(files)}
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
                { label: t("Favorites"), action: addFavouritesPane },
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
          disabled={isEditing || !mediaUploadSupported || maxAttachments === 0}
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
              role="combobox"
              aria-autocomplete="list"
              aria-expanded={Boolean(autocomplete)}
              aria-controls={autocomplete ? "compose-autocomplete-listbox" : undefined}
              aria-activedescendant={
                autocomplete?.items[autocomplete.selectedIndex]
                  ? `compose-autocomplete-option-${autocomplete.selectedIndex}`
                  : undefined
              }
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
              <ComposeAutocompleteListbox
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
          <ComposeAttachmentStrip
            attachments={attachments}
            onMove={moveAttachment}
            onRemove={removeAttachment}
          />
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
              disabled={
                isEditing || !mediaUploadSupported || maxAttachments === 0
              }
            >
              <Paperclip className="h-4 w-4" />
            </button>
            <button
              className={`btn btn-ghost btn-xs ${pollEnabled ? "bg-surface1 text-text" : ""}`}
              title={t("Poll")}
              onClick={() => setPollEnabled((current) => !current)}
              disabled={isEditing || !pollSupported}
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
              title={
                isEditing
                  ? t("Edit post ({shortcut})", {
                      shortcut: postShortcutLabel,
                    })
                  : t("Post ({shortcut})", { shortcut: postShortcutLabel })
              }
            >
              {posting ? (
                <Loader2 className="h-4 w-4 animate-spin" />
              ) : (
                <Send className="h-4 w-4" />
              )}
              {submitLabel}
            </button>
          </div>
        </div>
        <LiveRegion message={mediaAnnouncement} />
        {emojiOpen ? (
          <ComposeEmojiPicker
            customEmojis={customEmojis}
            unicodeEmojiCategories={unicodeEmojiCategories}
            onPickEmoji={insertComposeText}
          />
        ) : null}
      </div>
    </ComposeAreaView>
  );
}
