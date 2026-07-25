import { AtSign, Hash, Loader2, Smile } from "lucide-react";
import type {
  ComposeAutocompleteItem,
  ComposeAutocompleteKind,
} from "../../domain/composeAutocomplete";
import { t } from "../../i18n";
import { Avatar } from "../../components/common/Avatar";
import { RetriedCustomEmojiImage } from "../../components/common/CustomEmoji";
import {
  Listbox,
  ListboxOption,
} from "../../components/primitives/Listbox";

export function ComposeAutocompleteListbox({
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
    <Listbox
      id="compose-autocomplete-listbox"
      label={t("Search results")}
      busy={loading}
      className="absolute left-0 top-full z-30 mt-1 w-[min(360px,100%)] overflow-hidden rounded-md border border-surface0 bg-base-100 shadow-xl"
    >
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
              <ListboxOption
                key={`${kind}-${item.value}-${index}`}
                id={`compose-autocomplete-option-${index}`}
                selected={selected}
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
              </ListboxOption>
            );
          })}
        </div>
      )}
    </Listbox>
  );
}

