/**
 * CodeMirror 6, themed from the design tokens. Loaded on demand (it's the
 * largest piece of the file manager). Remount with a new `key` to load a
 * different document.
 */
import { defaultKeymap, history, historyKeymap, indentWithTab } from "@codemirror/commands";
import { json } from "@codemirror/lang-json";
import { yaml } from "@codemirror/lang-yaml";
import {
  bracketMatching,
  HighlightStyle,
  indentOnInput,
  StreamLanguage,
  syntaxHighlighting,
} from "@codemirror/language";
import { properties } from "@codemirror/legacy-modes/mode/properties";
import { toml } from "@codemirror/legacy-modes/mode/toml";
import { highlightSelectionMatches, searchKeymap } from "@codemirror/search";
import { EditorState, type Extension, type Text } from "@codemirror/state";
import {
  drawSelection,
  EditorView,
  highlightActiveLine,
  highlightActiveLineGutter,
  keymap,
  lineNumbers,
} from "@codemirror/view";
import { tags } from "@lezer/highlight";
import { useEffect, useImperativeHandle, useRef } from "react";

import type { EditorLanguage } from "./paths";

export interface CodeEditorHandle {
  /** The current text. */
  value(): string;
  /** Marks the current text as saved. */
  markSaved(): void;
}

const theme = EditorView.theme(
  {
    "&": {
      color: "var(--color-console-text)",
      backgroundColor: "var(--color-console)",
      height: "100%",
      fontSize: "12px",
    },
    ".cm-scroller": { fontFamily: "var(--font-mono)", lineHeight: "1.7" },
    ".cm-content": { caretColor: "var(--color-primary)", padding: "12px 0" },
    ".cm-cursor, .cm-dropCursor": { borderLeftColor: "var(--color-primary)" },
    "&.cm-focused": { outline: "none" },
    "&.cm-focused .cm-selectionBackground, .cm-selectionBackground, ::selection": {
      backgroundColor: "var(--color-console-selection)",
    },
    ".cm-gutters": {
      backgroundColor: "var(--color-console)",
      color: "var(--color-console-dim)",
      border: "none",
      paddingLeft: "8px",
    },
    ".cm-activeLine": { backgroundColor: "color-mix(in oklab, var(--color-surface) 55%, transparent)" },
    ".cm-activeLineGutter": { backgroundColor: "transparent", color: "var(--color-text-secondary)" },
    ".cm-matchingBracket": { backgroundColor: "var(--color-console-selection)", outline: "none" },
    ".cm-searchMatch": {
      backgroundColor: "var(--color-primary-subtle)",
      outline: "1px solid var(--color-primary)",
    },
    ".cm-panels": {
      backgroundColor: "var(--color-surface)",
      color: "var(--color-text-primary)",
      borderColor: "var(--color-border)",
    },
    ".cm-panel input, .cm-panel button, .cm-textfield": {
      fontFamily: "var(--font-sans)",
      fontSize: "12px",
      backgroundColor: "var(--color-field)",
      color: "var(--color-text-primary)",
      border: "1px solid var(--color-border-strong)",
      borderRadius: "5px",
    },
    ".cm-button": { backgroundImage: "none" },
  },
  { dark: true },
);

const highlight = HighlightStyle.define([
  {
    tag: [tags.comment, tags.lineComment, tags.blockComment],
    color: "var(--color-console-dim)",
    fontStyle: "italic",
  },
  {
    tag: [tags.propertyName, tags.attributeName, tags.definition(tags.propertyName)],
    color: "var(--color-ansi-bright-blue)",
  },
  { tag: [tags.string, tags.special(tags.string)], color: "var(--color-console-success)" },
  { tag: [tags.number, tags.integer, tags.float], color: "var(--color-console-warning)" },
  { tag: [tags.bool, tags.null, tags.atom, tags.keyword], color: "var(--color-ansi-bright-magenta)" },
  { tag: [tags.heading, tags.typeName, tags.className], color: "var(--color-nav-active-foreground)" },
  { tag: [tags.punctuation, tags.separator, tags.bracket], color: "var(--color-text-secondary)" },
  { tag: tags.invalid, color: "var(--color-console-danger)" },
]);

function languageExtension(language: EditorLanguage): Extension {
  switch (language) {
    case "json":
      return json();
    case "yaml":
      return yaml();
    case "properties":
      return StreamLanguage.define(properties);
    case "toml":
      return StreamLanguage.define(toml);
    default:
      return [];
  }
}

export default function CodeEditor({
  ref,
  initial,
  language,
  label,
  readOnly = false,
  onDirtyChange,
  onSave,
}: {
  ref?: React.Ref<CodeEditorHandle>;
  initial: string;
  language: EditorLanguage;
  /** Accessible name for the editing area. */
  label: string;
  readOnly?: boolean;
  onDirtyChange?: (dirty: boolean) => void;
  onSave?: () => void;
}) {
  const host = useRef<HTMLDivElement>(null);
  const view = useRef<EditorView | null>(null);
  const saved = useRef<Text | null>(null);
  const callbacks = useRef({ onDirtyChange, onSave });
  callbacks.current = { onDirtyChange, onSave };

  useImperativeHandle(ref, () => ({
    value: () => view.current?.state.doc.toString() ?? initial,
    markSaved: () => {
      if (!view.current) return;
      saved.current = view.current.state.doc;
      callbacks.current.onDirtyChange?.(false);
    },
  }));

  // biome-ignore lint/correctness/useExhaustiveDependencies: the document loads once; callers remount with a new key for another
  useEffect(() => {
    if (!host.current) return;
    const state = EditorState.create({
      doc: initial,
      extensions: [
        lineNumbers(),
        highlightActiveLineGutter(),
        highlightActiveLine(),
        drawSelection(),
        history(),
        indentOnInput(),
        bracketMatching(),
        highlightSelectionMatches(),
        syntaxHighlighting(highlight),
        languageExtension(language),
        theme,
        EditorState.readOnly.of(readOnly),
        EditorView.contentAttributes.of({ "aria-label": label }),
        keymap.of([
          {
            key: "Mod-s",
            preventDefault: true,
            run: () => {
              callbacks.current.onSave?.();
              return true;
            },
          },
          ...defaultKeymap,
          ...historyKeymap,
          ...searchKeymap,
          indentWithTab,
        ]),
        EditorView.updateListener.of((update) => {
          if (update.docChanged && saved.current) {
            callbacks.current.onDirtyChange?.(!update.state.doc.eq(saved.current));
          }
        }),
      ],
    });
    const editor = new EditorView({ state, parent: host.current });
    view.current = editor;
    saved.current = state.doc;
    return () => {
      editor.destroy();
      view.current = null;
    };
  }, []);

  return <div ref={host} className="h-full min-h-0 overflow-hidden" />;
}
