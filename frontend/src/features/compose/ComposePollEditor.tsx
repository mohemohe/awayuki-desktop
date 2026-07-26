import React from "react";
import { ChevronDown, X } from "lucide-react";
import { pollDurations, pollDurationLabel } from "../../constants/compose";
import { appLocale, t } from "../../i18n";

const pollDurationDisplayLabel = (seconds: number) => {
  const duration = pollDurations.find((item) => item.seconds === seconds);
  if (!duration) return pollDurationLabel(seconds);
  return appLocale === "ja" ? duration.labelJa : duration.label;
};

export function ComposePollEditor({
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
            className={`btn join-item btn-xs h-7 min-h-7 border-blue bg-blue px-2 text-xs text-primary-content hover:border-sapphire hover:bg-sapphire hover:text-primary-content ${!multiple ? "btn-active" : ""}`}
            onClick={() => onMultipleChange(false)}
          >
            {t("Single")}
          </button>
          <button
            className={`btn join-item btn-xs h-7 min-h-7 border-blue bg-blue px-2 text-xs text-primary-content hover:border-sapphire hover:bg-sapphire hover:text-primary-content ${multiple ? "btn-active" : ""}`}
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
