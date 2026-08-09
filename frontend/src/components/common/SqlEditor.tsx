import React from "react";
import { sql, SQLite } from "@codemirror/lang-sql";
import {
  HighlightStyle,
  type StreamParser,
  StreamLanguage,
  type StringStream,
  syntaxHighlighting,
} from "@codemirror/language";
import { clojure as yq } from "@codemirror/legacy-modes/mode/clojure";
import { css } from "@codemirror/legacy-modes/mode/css";
import { Compartment, type Extension } from "@codemirror/state";
import { tags } from "@lezer/highlight";
import { EditorView, basicSetup } from "codemirror";
import { useAppStore } from "../../store/appStore";

type CodeMirrorEditorProps = {
  value: string;
  onChange: (value: string) => void;
  language: Extension;
  ariaLabel: string;
  className?: string;
};

type QueryEditorProps = Omit<CodeMirrorEditorProps, "language" | "ariaLabel">;

const queryHighlightStyle = HighlightStyle.define([
  { tag: tags.keyword, color: "rgb(var(--ctp-blue))", fontWeight: "600" },
  {
    tag: tags.operatorKeyword,
    color: "rgb(var(--ctp-blue))",
    fontWeight: "600",
  },
  { tag: tags.operator, color: "rgb(var(--ctp-blue))" },
  { tag: tags.standard(tags.variableName), color: "rgb(var(--ctp-blue))" },
  { tag: tags.string, color: "rgb(var(--ctp-green))" },
  { tag: tags.number, color: "rgb(var(--ctp-peach))" },
  { tag: tags.bool, color: "rgb(var(--ctp-peach))" },
  { tag: tags.atom, color: "rgb(var(--ctp-peach))" },
  { tag: tags.variableName, color: "rgb(var(--ctp-text))" },
  {
    tag: tags.function(tags.variableName),
    color: "rgb(var(--ctp-mauve))",
  },
  { tag: tags.propertyName, color: "rgb(var(--ctp-yellow))" },
  {
    tag: tags.comment,
    color: "rgb(var(--ctp-overlay0))",
    fontStyle: "italic",
  },
  { tag: tags.punctuation, color: "rgb(var(--ctp-subtext1))" },
  { tag: tags.bracket, color: "rgb(var(--ctp-subtext1))" },
]);

const createQueryEditorTheme = (dark: boolean) =>
  EditorView.theme(
    {
      "&": {
        minHeight: "18rem",
        width: "100%",
        border: "1px solid rgb(var(--ctp-surface0))",
        borderRadius: "0.5rem",
        backgroundColor: "rgb(var(--ctp-mantle))",
        color: "rgb(var(--ctp-text))",
        fontSize: "0.875rem",
      },
      "&.cm-focused": {
        outline: "2px solid rgb(var(--ctp-blue))",
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
        borderRight: "1px solid rgb(var(--ctp-surface0))",
        backgroundColor: "rgb(var(--ctp-crust))",
        color: "rgb(var(--ctp-overlay0))",
      },
      ".cm-activeLine": {
        backgroundColor: "rgb(var(--ctp-surface0) / 0.4)",
      },
      ".cm-activeLineGutter": {
        backgroundColor: "rgb(var(--ctp-surface0))",
        color: "rgb(var(--ctp-subtext1))",
      },
      ".cm-selectionBackground": {
        backgroundColor: "rgb(var(--ctp-surface1)) !important",
      },
      ".cm-cursor": {
        borderLeftColor: "rgb(var(--ctp-text))",
      },
    },
    { dark },
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
  const initialValueRef = React.useRef(value);
  const themeCompartmentRef = React.useRef(new Compartment());
  const theme = useAppStore(
    (state) => state.snapshot?.settings.appearance?.theme ?? "Mocha",
  );
  const initialThemeRef = React.useRef(theme);

  React.useEffect(() => {
    onChangeRef.current = onChange;
  }, [onChange]);

  React.useEffect(() => {
    if (!containerRef.current) return;

    const view = new EditorView({
      doc: initialValueRef.current,
      parent: containerRef.current,
      extensions: [
        basicSetup,
        language,
        EditorView.lineWrapping,
        themeCompartmentRef.current.of(
          createQueryEditorTheme(initialThemeRef.current !== "Latte"),
        ),
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
  }, [ariaLabel, language]);

  React.useEffect(() => {
    const view = viewRef.current;
    if (!view) return;
    view.dispatch({
      effects: themeCompartmentRef.current.reconfigure(
        createQueryEditorTheme(theme !== "Latte"),
      ),
    });
  }, [theme]);

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

const sqlLanguage = sql({ dialect: SQLite });
const yqLanguage = StreamLanguage.define(yq);

type KqHighlightState = { inString: boolean; inSourceList: boolean };

// KQ is an infix language, so the Clojure-based YQ mode cannot classify it.
const kqWordOperators = new Set([
  "and",
  "or",
  "not",
  "contains",
  "in",
  "startswith",
  "startwith",
  "endswith",
  "endwith",
  "match",
  "regex",
  "caseful",
]);

const kqSources = new Set([
  "local",
  "all",
  "home",
  "mention",
  "mentions",
  "reply",
  "replies",
  "message",
  "messages",
  "dm",
  "dms",
  "direct",
  "list",
  "search",
  "find",
  "track",
  "stream",
  "conv",
  "conversation",
  "talk",
  "tree",
  "user",
  "public",
  "federated",
  "local_public",
  "localpublic",
  "hashtag",
  "tag",
  "bookmarks",
  "bookmarked",
  "favourites",
  "favorites",
  "favs",
]);

function consumeKqString(stream: StringStream, state: KqHighlightState) {
  while (!stream.eol()) {
    const character = stream.next();
    if (character === "\\") {
      stream.next();
    } else if (character === '"') {
      state.inString = false;
      break;
    }
  }
  return "string";
}

const kqParser: StreamParser<KqHighlightState> = {
  name: "krile-query",
  startState: () => ({ inString: false, inSourceList: false }),
  token(stream, state) {
    if (state.inString) return consumeKqString(stream, state);
    if (stream.eatSpace()) return null;
    if (stream.peek() === '"') {
      stream.next();
      state.inString = true;
      return consumeKqString(stream, state);
    }
    if (stream.match(/^@"(?:\\["\\]|[^"])*"/)) return "string";
    if (stream.match(/^@[^\s()[\],!*/+&|<>="]+/)) return "string";
    if (stream.match(/^#"(?:\\["\\]|[^"])*"/)) return "string";
    if (stream.match(/^#[0-9]+/)) return "string";
    if (stream.match(/^\d+/)) return "number";
    if (stream.match(/^(?:&&|\|\||==|!=|<=|>=|<-|->|[!*/+\-&|<>=])/)) {
      if (state.inSourceList && stream.current() === "*") return "atom";
      return "operator";
    }
    if (stream.match(/^[()[\],.:]/)) return "punctuation";
    const word = stream.match(/^[^\s()[\],.:!*/+\-&|<>="]+/);
    if (word) {
      const value = stream.current().toLowerCase();
      if (value === "from") {
        state.inSourceList = true;
        return "keyword";
      }
      if (value === "where") {
        state.inSourceList = false;
        return "keyword";
      }
      if (kqWordOperators.has(value)) return "operator";
      if (state.inSourceList && kqSources.has(value)) return "atom";
      return "variableName";
    }
    stream.next();
    return null;
  },
};

const kqLanguage = StreamLanguage.define(kqParser);

export function SqlEditor(props: QueryEditorProps) {
  return (
    <CodeMirrorEditor
      {...props}
      ariaLabel="SQL"
      language={sqlLanguage}
    />
  );
}

export function YqEditor(props: QueryEditorProps) {
  return (
    <CodeMirrorEditor
      {...props}
      ariaLabel="YQ"
      language={yqLanguage}
    />
  );
}

export function KqEditor(props: QueryEditorProps) {
  return (
    <CodeMirrorEditor
      {...props}
      ariaLabel="KQ"
      language={kqLanguage}
    />
  );
}

export function CssEditor(props: QueryEditorProps) {
  return (
    <CodeMirrorEditor
      {...props}
      ariaLabel="CSS"
      language={StreamLanguage.define(css)}
    />
  );
}
