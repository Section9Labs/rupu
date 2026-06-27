// CodeEditorImpl — the CodeMirror 6 body behind the lazy `CodeEditor` wrapper.
//
// Built directly on `EditorView` / `EditorState` (no React wrapper dep). This
// module statically imports the `@codemirror/*` packages; it is ONLY ever
// pulled in via `React.lazy(() => import('./CodeEditorImpl'))`, so Vite emits
// it (and CodeMirror) as a separate async chunk, keeping the main bundle lean.
//
// The extension set is a hand-rolled equivalent of CodeMirror's `basicSetup`
// (minus search/lint): real syntax highlighting, an active-line + fold gutter,
// bracket matching/closing, autocompletion, and Tab-to-indent — so editing an
// agent `.md` or workflow `.yaml` feels like a proper code editor, not a
// textarea. Tuned for the app's light UI (`panel: #fff`).

import { useEffect, useRef } from 'react';
import { EditorState, type Extension } from '@codemirror/state';
import {
  EditorView,
  keymap,
  lineNumbers,
  highlightActiveLine,
  highlightActiveLineGutter,
  highlightSpecialChars,
  drawSelection,
  dropCursor,
  rectangularSelection,
  crosshairCursor,
} from '@codemirror/view';
import {
  defaultKeymap,
  history,
  historyKeymap,
  indentWithTab,
} from '@codemirror/commands';
import {
  syntaxHighlighting,
  defaultHighlightStyle,
  bracketMatching,
  indentOnInput,
  foldGutter,
  foldKeymap,
} from '@codemirror/language';
import {
  autocompletion,
  completionKeymap,
  closeBrackets,
  closeBracketsKeymap,
} from '@codemirror/autocomplete';
import { markdown } from '@codemirror/lang-markdown';
import { yaml } from '@codemirror/lang-yaml';
import type { CodeEditorProps } from './CodeEditor';

const CONTAINER_CLASS =
  'overflow-hidden rounded-xl border border-border bg-panel text-[12px] ' +
  'leading-relaxed shadow-card focus-within:border-brand-500';

function languageExtension(language: 'markdown' | 'yaml'): Extension {
  return language === 'yaml' ? yaml() : markdown();
}

// A small light theme on top of `defaultHighlightStyle` (which supplies the
// token colors). Keeps the gutter/active-line subtle against the white panel.
const editorTheme = EditorView.theme({
  '&': { maxHeight: '32rem' },
  '.cm-scroller': { fontFamily: 'inherit', overflow: 'auto' },
  '.cm-content': { padding: '0.5rem 0' },
  '.cm-gutters': {
    backgroundColor: 'transparent',
    color: '#9ca3af',
    border: 'none',
  },
  '.cm-activeLine': { backgroundColor: 'rgba(59, 130, 246, 0.06)' },
  '.cm-activeLineGutter': { backgroundColor: 'rgba(59, 130, 246, 0.06)' },
  '&.cm-focused .cm-matchingBracket': {
    backgroundColor: 'rgba(59, 130, 246, 0.18)',
    outline: 'none',
  },
});

/** The full "rich editor" extension set, sans the language (added per-mode). */
function baseExtensions(): Extension[] {
  return [
    lineNumbers(),
    highlightActiveLineGutter(),
    highlightSpecialChars(),
    history(),
    foldGutter(),
    drawSelection(),
    dropCursor(),
    indentOnInput(),
    syntaxHighlighting(defaultHighlightStyle, { fallback: true }),
    bracketMatching(),
    closeBrackets(),
    autocompletion(),
    rectangularSelection(),
    crosshairCursor(),
    highlightActiveLine(),
    EditorView.lineWrapping,
    keymap.of([
      ...closeBracketsKeymap,
      ...defaultKeymap,
      ...historyKeymap,
      ...foldKeymap,
      ...completionKeymap,
      indentWithTab,
    ]),
    editorTheme,
  ];
}

export default function CodeEditorImpl({
  value,
  onChange,
  language = 'markdown',
  ariaLabel,
}: CodeEditorProps) {
  const containerRef = useRef<HTMLDivElement>(null);
  const viewRef = useRef<EditorView | null>(null);
  // Keep the latest onChange without re-creating the editor on every render.
  const onChangeRef = useRef(onChange);
  onChangeRef.current = onChange;

  // Create the editor on mount (and re-create only when the language changes).
  useEffect(() => {
    const parent = containerRef.current;
    if (!parent) return;

    const updateListener = EditorView.updateListener.of((u) => {
      if (u.docChanged) onChangeRef.current(u.state.doc.toString());
    });

    const state = EditorState.create({
      doc: value,
      extensions: [
        ...baseExtensions(),
        languageExtension(language),
        updateListener,
      ],
    });

    const view = new EditorView({ state, parent });
    if (ariaLabel) view.contentDOM.setAttribute('aria-label', ariaLabel);
    viewRef.current = view;

    return () => {
      view.destroy();
      viewRef.current = null;
    };
    // `value` is intentionally omitted — external value sync is handled below;
    // recreating on every keystroke would reset the cursor.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [language, ariaLabel]);

  // Sync external `value` changes (e.g. a reset to the saved definition) into
  // the doc without clobbering local edits when they already match.
  useEffect(() => {
    const view = viewRef.current;
    if (!view) return;
    const current = view.state.doc.toString();
    if (value !== current) {
      view.dispatch({ changes: { from: 0, to: current.length, insert: value } });
    }
  }, [value]);

  return <div ref={containerRef} className={CONTAINER_CLASS} />;
}
