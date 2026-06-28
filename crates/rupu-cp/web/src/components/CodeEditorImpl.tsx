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
import { highlightStyleFor } from './codeHighlightTheme';
import type { CodeEditorProps } from './CodeEditor';
import type { Mode } from './theme/ThemeProvider';

// `font-mono` so the editor matches the read-only display (CodeHighlight), whose
// <pre> is font-mono — `.cm-scroller` inherits this family below.
const CONTAINER_CLASS =
  'overflow-hidden rounded-xl border border-border bg-panel font-mono text-ui ' +
  'leading-relaxed shadow-card focus-within:border-brand-500';

function languageExtension(language: 'markdown' | 'yaml'): Extension {
  return language === 'yaml' ? yaml() : markdown();
}

// A small editor theme on top of the syntax highlight style (which supplies the
// token colors). Keeps the gutter/active-line subtle against the panel. Tuned
// per mode so dark mode isn't light-grey-gutter-on-near-black; the container's
// `bg-panel` provides the surface, so the editor background stays transparent.
function makeEditorTheme(dark: boolean): Extension {
  return EditorView.theme(
    {
      '&': { maxHeight: '32rem', backgroundColor: 'transparent', color: 'inherit' },
      '.cm-scroller': { fontFamily: 'inherit', overflow: 'auto' },
      '.cm-content': { padding: '0.5rem 0', caretColor: dark ? '#e6edf3' : '#0f172a' },
      '.cm-cursor, .cm-dropCursor': { borderLeftColor: dark ? '#e6edf3' : '#0f172a' },
      '.cm-gutters': {
        backgroundColor: 'transparent',
        color: dark ? '#6b7280' : '#9ca3af',
        border: 'none',
      },
      '.cm-activeLine': { backgroundColor: dark ? 'rgba(96, 165, 250, 0.10)' : 'rgba(59, 130, 246, 0.06)' },
      '.cm-activeLineGutter': {
        backgroundColor: dark ? 'rgba(96, 165, 250, 0.10)' : 'rgba(59, 130, 246, 0.06)',
      },
      '.cm-selectionBackground, &.cm-focused .cm-selectionBackground, ::selection': {
        backgroundColor: dark ? 'rgba(96, 165, 250, 0.25)' : 'rgba(59, 130, 246, 0.18)',
      },
      '&.cm-focused .cm-matchingBracket': {
        backgroundColor: dark ? 'rgba(96, 165, 250, 0.30)' : 'rgba(59, 130, 246, 0.18)',
        outline: 'none',
      },
    },
    { dark },
  );
}

/** The full "rich editor" extension set, sans the language (added per-mode). */
function baseExtensions(mode: Mode): Extension[] {
  const dark = mode === 'dark';
  return [
    lineNumbers(),
    highlightActiveLineGutter(),
    highlightSpecialChars(),
    history(),
    foldGutter(),
    drawSelection(),
    dropCursor(),
    indentOnInput(),
    syntaxHighlighting(highlightStyleFor(mode), { fallback: true }),
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
    makeEditorTheme(dark),
  ];
}

/** Resolve the effective mode: explicit prop wins, else read the live
 *  `data-theme` attribute at mount (the editor mounts imperatively). */
function resolveMode(theme: Mode | undefined): Mode {
  if (theme) return theme;
  try {
    return document.documentElement.dataset.theme === 'dark' ? 'dark' : 'light';
  } catch {
    return 'light';
  }
}

export default function CodeEditorImpl({
  value,
  onChange,
  language = 'markdown',
  ariaLabel,
  theme,
}: CodeEditorProps) {
  const mode = resolveMode(theme);
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
        ...baseExtensions(mode),
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
    // recreating on every keystroke would reset the cursor. `mode` IS included so
    // the editor recreates with the matching highlight/theme on a theme toggle.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [language, ariaLabel, mode]);

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
