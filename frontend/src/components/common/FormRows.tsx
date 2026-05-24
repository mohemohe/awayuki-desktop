import { ChevronDown } from "lucide-react";

export function SelectRow<T extends string>({
  label,
  value,
  values,
  optionLabel,
  onChange,
}: {
  label: string;
  value: T;
  values: readonly T[];
  optionLabel?: (value: T) => string;
  onChange: (value: T) => void;
}) {
  return (
    <label className="contents">
      <span className="self-center text-sm text-subtext0">{label}</span>
      <span className="relative inline-flex max-w-xs">
        <select
          className="select select-bordered select-sm h-8 min-h-8 w-full appearance-none border-surface0 bg-base-200 bg-none pr-8 text-sm"
          value={value}
          onChange={(event) => onChange(event.target.value as T)}
        >
          {values.map((item) => (
            <option key={item} value={item}>
              {optionLabel ? optionLabel(item) : item}
            </option>
          ))}
        </select>
        <ChevronDown className="pointer-events-none absolute right-3 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-subtext0" />
      </span>
    </label>
  );
}

export function ToggleRow({
  label,
  checked,
  onChange,
  disabled = false,
}: {
  label: string;
  checked: boolean;
  onChange: (checked: boolean) => void;
  disabled?: boolean;
}) {
  return (
    <label className="contents">
      <span className="self-center text-sm text-subtext0">{label}</span>
      <input
        type="checkbox"
        className="toggle toggle-primary toggle-sm"
        checked={checked}
        disabled={disabled}
        onChange={(event) => onChange(event.target.checked)}
      />
    </label>
  );
}
