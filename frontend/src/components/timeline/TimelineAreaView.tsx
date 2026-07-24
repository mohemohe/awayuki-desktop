import React from "react";

export type TimelineAreaViewPane = {
  paneIndex: number;
  content: React.ReactNode;
};

export function TimelineAreaView({
  panes,
  onRender,
}: {
  panes: TimelineAreaViewPane[];
  onRender?: React.ProfilerProps["onRender"];
}) {
  return (
    <div className="min-h-0 flex-1 overflow-x-auto bg-base-200">
      <div className="flex h-full min-w-full">
        {panes.map(({ paneIndex, content }) =>
          onRender ? (
            <React.Profiler
              id={`timeline:scroll:pane-${paneIndex}`}
              key={paneIndex}
              onRender={onRender}
            >
              {content}
            </React.Profiler>
          ) : (
            <React.Fragment key={paneIndex}>{content}</React.Fragment>
          ),
        )}
      </div>
    </div>
  );
}

export function TimelinePaneView({
  paneIndex,
  header,
  children,
}: {
  paneIndex: number;
  header: React.ReactNode;
  children: React.ReactNode;
}) {
  return (
    <section
      className="flex h-full min-w-[360px] flex-1 flex-col border-r border-surface0 bg-base"
      data-pane-index={paneIndex}
    >
      <div className="flex h-8 shrink-0 items-stretch border-b border-surface0 bg-base-300">
        {header}
      </div>
      {children}
    </section>
  );
}

export function EmptyTimelinePaneView({
  title,
  message,
}: {
  title: string;
  message: string;
}) {
  return (
    <section className="flex h-full min-w-[360px] flex-1 flex-col border-r border-surface0 bg-base">
      <div className="flex h-8 shrink-0 items-center border-b border-surface0 bg-base-300 px-2 text-xs text-subtext0">
        {title}
      </div>
      <div className="grid flex-1 place-items-center text-xs text-subtext0">
        {message}
      </div>
    </section>
  );
}
