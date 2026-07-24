import React from "react";

export function ComposeAreaView({
  sectionRef,
  height,
  mediaDropEnabled,
  onDropFiles,
  children,
}: {
  sectionRef?: React.Ref<HTMLElement>;
  height: number;
  mediaDropEnabled: boolean;
  onDropFiles: (files: File[]) => void;
  children: React.ReactNode;
}) {
  return (
    <section
      ref={sectionRef}
      className="grid shrink-0 grid-cols-[52px_minmax(0,1fr)] overflow-visible border-b border-surface0 bg-base"
      style={{ height }}
      onDragOver={(event) => {
        event.preventDefault();
        event.dataTransfer.dropEffect = mediaDropEnabled ? "copy" : "none";
      }}
      onDrop={(event) => {
        event.preventDefault();
        const files = Array.from(event.dataTransfer.files);
        if (files.length > 0) onDropFiles(files);
      }}
    >
      {children}
    </section>
  );
}
