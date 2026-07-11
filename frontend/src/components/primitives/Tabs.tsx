import React from "react";

export function TabList({
  label,
  orientation = "horizontal",
  children,
  className,
}: {
  label: string;
  orientation?: "horizontal" | "vertical";
  children: React.ReactNode;
  className?: string;
}) {
  const ref = React.useRef<HTMLDivElement>(null);
  const onKeyDown = (event: React.KeyboardEvent) => {
    const previousKey = orientation === "vertical" ? "ArrowUp" : "ArrowLeft";
    const nextKey = orientation === "vertical" ? "ArrowDown" : "ArrowRight";
    if (![previousKey, nextKey, "Home", "End"].includes(event.key)) return;
    const tabs = Array.from(
      ref.current?.querySelectorAll<HTMLButtonElement>(
        '[role="tab"]:not([disabled])',
      ) ?? [],
    );
    if (tabs.length === 0) return;
    event.preventDefault();
    const current = tabs.indexOf(document.activeElement as HTMLButtonElement);
    const next =
      event.key === "Home"
        ? 0
        : event.key === "End"
          ? tabs.length - 1
          : (current + (event.key === nextKey ? 1 : -1) + tabs.length) %
            tabs.length;
    tabs[next]?.focus();
    tabs[next]?.click();
  };
  return (
    <div
      ref={ref}
      role="tablist"
      aria-label={label}
      aria-orientation={orientation}
      className={className}
      onKeyDown={onKeyDown}
    >
      {children}
    </div>
  );
}

export function Tab({
  selected,
  controls,
  id,
  className,
  title,
  onSelect,
  children,
}: {
  selected: boolean;
  controls?: string;
  id?: string;
  className?: string;
  title?: string;
  onSelect: () => void;
  children: React.ReactNode;
}) {
  return (
    <button
      type="button"
      role="tab"
      id={id}
      aria-selected={selected}
      aria-controls={controls}
      tabIndex={selected ? 0 : -1}
      className={className}
      title={title}
      onClick={onSelect}
    >
      {children}
    </button>
  );
}
