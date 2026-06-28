// CodeEditor — a controlled code editor for editing agent / workflow
// definitions in the browser.
//
// The heavy CodeMirror 6 wiring lives in `CodeEditorImpl`, loaded via
// `React.lazy` so the `@codemirror/*` packages land in their own async chunk
// and never bloat the main `index-*.js` entry. Until that chunk resolves
// (and as a graceful fallback if it never does) a plain controlled
// `<textarea>` with the same external API is shown — so editing always works.

import { lazy, Suspense, useContext } from 'react';
import { ThemeContext, type Mode } from './theme/ThemeProvider';

export interface CodeEditorProps {
  value: string;
  onChange: (v: string) => void;
  language?: 'markdown' | 'yaml';
  ariaLabel?: string;
  /** Resolved theme mode — injected by the wrapper so the imperative CodeMirror
   *  view picks the matching highlight + editor theme and reconfigures on toggle. */
  theme?: Mode;
}

const CodeEditorImpl = lazy(() => import('./CodeEditorImpl'));

const FALLBACK_CLASS =
  'block w-full min-h-[20rem] resize-y rounded-xl border border-border bg-panel ' +
  'p-4 font-mono text-ui leading-relaxed text-ink focus:border-brand-500 ' +
  'focus:outline-none';

function FallbackEditor({ value, onChange, ariaLabel }: CodeEditorProps) {
  return (
    <textarea
      value={value}
      onChange={(e) => onChange(e.target.value)}
      aria-label={ariaLabel}
      spellCheck={false}
      className={FALLBACK_CLASS}
    />
  );
}

export default function CodeEditor(props: CodeEditorProps) {
  // Provider-optional: fall back to undefined (the impl reads `data-theme`) so
  // the editor renders in isolated tests without a ThemeProvider.
  const mode = useContext(ThemeContext)?.mode;
  return (
    <Suspense fallback={<FallbackEditor {...props} />}>
      <CodeEditorImpl {...props} theme={mode} />
    </Suspense>
  );
}
