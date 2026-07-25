import React from "react";

export function Listbox({
  id,
  label,
  busy,
  className,
  children,
}: {
  id: string;
  label: string;
  busy?: boolean;
  className?: string;
  children: React.ReactNode;
}) {
  return (
    <div
      id={id}
      role="listbox"
      aria-label={label}
      aria-busy={busy || undefined}
      className={className}
    >
      {children}
    </div>
  );
}

export function ListboxOption({
  id,
  selected,
  className,
  onMouseEnter,
  onMouseDown,
  children,
}: {
  id: string;
  selected: boolean;
  className?: string;
  onMouseEnter?: () => void;
  onMouseDown?: (event: React.MouseEvent<HTMLButtonElement>) => void;
  children: React.ReactNode;
}) {
  return (
    <button
      id={id}
      type="button"
      role="option"
      aria-selected={selected}
      tabIndex={-1}
      className={className}
      onMouseEnter={onMouseEnter}
      onMouseDown={onMouseDown}
    >
      {children}
    </button>
  );
}

export function LiveRegion({ message }: { message: string }) {
  return (
    <span className="sr-only" role="status" aria-live="polite" aria-atomic="true">
      {message}
    </span>
  );
}

