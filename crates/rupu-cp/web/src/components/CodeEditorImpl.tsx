// CodeEditorImpl — the CodeMirror 6 body behind the lazy `CodeEditor` wrapper.
//
// Built directly on `EditorView` / `EditorState` (no React wrapper dep). This
// module statically imports the `@codemirror/*` packages; it is ONLY ever
// pulled in via `React.lazy(() => import('./CodeEditorImpl'))`, so Vite emits
// it (and CodeMirror) as a separate async chunk, keeping the main bundle lean.

import { useEffect, useRef } from 'react';
import { EditorState, type Extension } from '@codemirror/state';
import { EditorView, keymap, lineNumbers } from '@codemirror/view';
import { defaultKeymap, history, historyKeymap } from '@codemirror/commands';
import { markdown } from '@codemirror/lang-markdown';
import { yaml } from '@codemirror/lang-yaml';
import type { CodeEditorProps } from './CodeEditor';

const CONTAINER_CLASS =
  'overflow-hidden rounded-xl border border-border bg-panel text-[12px] ' +
  'leading-relaxed shadow-card focus-within:border-brand-500';

function languageExtension(language: 'markdown' | 'yaml'): Extension {
  return language === 'yaml' ? yaml() : markdown();
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
        lineNumbers(),
        history(),
        keymap.of([...defaultKeymap, ...historyKeymap]),
        languageExtension(language),
        EditorView.lineWrapping,
        updateListener,
        EditorView.theme({
          '&': { maxHeight: '32rem' },
          '.cm-scroller': { fontFamily: 'inherit', overflow: 'auto' },
          '.cm-content': { padding: '0.75rem 0' },
        }),
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
