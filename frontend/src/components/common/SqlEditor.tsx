import React from "react";
import { sql, SQLite } from "@codemirror/lang-sql";
import {
  HighlightStyle,
  StreamLanguage,
  syntaxHighlighting,
  type LanguageSupport,
} from "@codemirror/language";
import { clojure as yq } from "@codemirror/legacy-modes/mode/clojure";
import { tags } from "@lezer/highlight";
import { EditorView, basicSetup } from "codemirror";

type CodeMirrorEditorProps = {
  value: string;
  onChange: (value: string) => void;
  language: LanguageSupport;
  ariaLabel: string;
  className?: string;
};

type QueryEditorProps = Omit<CodeMirrorEditorProps, "language" | "ariaLabel">;

const queryHighlightStyle = HighlightStyle.define([
  { tag: tags.keyword, color: "#89b4fa", fontWeight: "600" },
  { tag: tags.operatorKeyword, color: "#89b4fa", fontWeight: "600" },
  { tag: tags.standard(tags.variableName), color: "#89b4fa" },
  { tag: tags.string, color: "#a6e3a1" },
  { tag: tags.number, color: "#fab387" },
  { tag: tags.bool, color: "#fab387" },
  { tag: tags.atom, color: "#fab387" },
  { tag: tags.variableName, color: "#cdd6f4" },
  { tag: tags.function(tags.variableName), color: "#cba6f7" },
  { tag: tags.propertyName, color: "#f9e2af" },
  { tag: tags.comment, color: "#6c7086", fontStyle: "italic" },
  { tag: tags.punctuation, color: "#bac2de" },
  { tag: tags.bracket, color: "#bac2de" },
]);

const queryEditorTheme = EditorView.theme(
  {
    "&": {
      minHeight: "18rem",
      width: "100%",
      border: "1px solid #313244",
      borderRadius: "0.5rem",
      backgroundColor: "#181825",
      color: "#cdd6f4",
      fontSize: "0.875rem",
    },
    "&.cm-focused": {
      outline: "2px solid #89b4fa",
      outlineOffset: "2px",
    },
    ".cm-scroller": {
      minHeight: "18rem",
      fontFamily:
        'ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, "Liberation Mono", "Courier New", monospace',
      lineHeight: "1.55",
    },
    ".cm-content": {
      boxSizing: "border-box",
      minHeight: "18rem",
      padding: "0.75rem 0.5rem",
      userSelect: "text",
      WebkitUserSelect: "text",
    },
    ".cm-line": {
      padding: "0 0.5rem",
    },
    ".cm-gutters": {
      borderRight: "1px solid #313244",
      backgroundColor: "#11111b",
      color: "#6c7086",
    },
    ".cm-activeLine": {
      backgroundColor: "#31324466",
    },
    ".cm-activeLineGutter": {
      backgroundColor: "#313244",
      color: "#bac2de",
    },
    ".cm-selectionBackground": {
      backgroundColor: "#45475a !important",
    },
    ".cm-cursor": {
      borderLeftColor: "#cdd6f4",
    },
  },
  { dark: true },
);

const focusEditorBlankArea = EditorView.domEventHandlers({
  mousedown(event, view) {
    const target = event.target;
    if (!(target instanceof HTMLElement)) return false;
    if (!target.closest(".cm-scroller")) return false;
    if (target.closest(".cm-content")) return false;

    event.preventDefault();
    view.focus();
    view.dispatch({
      selection: { anchor: view.state.doc.length },
      scrollIntoView: true,
    });
    return true;
  },
});

function CodeMirrorEditor({
  value,
  onChange,
  language,
  ariaLabel,
  className,
}: CodeMirrorEditorProps) {
  const containerRef = React.useRef<HTMLDivElement | null>(null);
  const viewRef = React.useRef<EditorView | null>(null);
  const onChangeRef = React.useRef(onChange);

  React.useEffect(() => {
    onChangeRef.current = onChange;
  }, [onChange]);

  React.useEffect(() => {
    if (!containerRef.current) return;

    const view = new EditorView({
      doc: value,
      parent: containerRef.current,
      extensions: [
        basicSetup,
        language,
        EditorView.lineWrapping,
        queryEditorTheme,
        focusEditorBlankArea,
        syntaxHighlighting(queryHighlightStyle),
        EditorView.contentAttributes.of({ "aria-label": ariaLabel }),
        EditorView.updateListener.of((update) => {
          if (!update.docChanged) return;
          onChangeRef.current(update.state.doc.toString());
        }),
      ],
    });

    viewRef.current = view;
    return () => {
      view.destroy();
      viewRef.current = null;
    };
  }, []);

  React.useEffect(() => {
    const view = viewRef.current;
    if (!view) return;
    const currentValue = view.state.doc.toString();
    if (currentValue === value) return;

    view.dispatch({
      changes: { from: 0, to: currentValue.length, insert: value },
    });
  }, [value]);

  return (
    <div
      className={`sql-editor min-w-0 ${className ?? ""}`}
      ref={containerRef}
    />
  );
}

export function SqlEditor(props: QueryEditorProps) {
  return (
    <CodeMirrorEditor
      {...props}
      ariaLabel="SQL"
      language={sql({ dialect: SQLite })}
    />
  );
}

export function YqEditor(props: QueryEditorProps) {
  return (
    <CodeMirrorEditor
      {...props}
      ariaLabel="YQ"
      language={StreamLanguage.define(yq)}
    />
  );
}
