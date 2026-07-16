/**
 * SourcePreview — line-numbered source-file slice for the transcript
 * drill-down's "view source" affordance (e.g. jump-to-line from a tool call
 * or finding's file:line reference).
 *
 * Fetches lazily via `api.readSource` on mount (and whenever `runId` / `path`
 * / `line` / `host` change) — callers control lazy mounting by not rendering
 * this component until the preview is opened.
 *
 * States:
 *   - loading    → "Loading source…"
 *   - error      → fetch/network failure message
 *   - unavailable (`available: false`) → the backend's `reason` text
 *   - loaded     → line-numbered slice, `targetLine` row emphasized,
 *                  syntax-highlighted via `CodeHighlight` when the slice's
 *                  `language` is one of the registered hljs grammars.
 */

import * as React from 'react';
import { api } from '../../lib/api';
import type { SourceSlice } from '../../lib/api';
import CodeHighlight, { SOURCE_PREVIEW_LANGUAGES } from '../CodeHighlight';

type SourcePreviewState =
  | { status: 'loading' }
  | { status: 'error'; message: string }
  | { status: 'loaded'; slice: SourceSlice };

export interface SourcePreviewProps {
  runId: string;
  path: string;
  line: number;
  host?: string;
}

export default function SourcePreview({ runId, path, line, host }: SourcePreviewProps) {
  const [state, setState] = React.useState<SourcePreviewState>({ status: 'loading' });

  React.useEffect(() => {
    let alive = true;
    setState({ status: 'loading' });
    api
      .readSource(runId, path, line, { host })
      .then((slice) => {
        if (alive) setState({ status: 'loaded', slice });
      })
      .catch((e) => {
        if (alive) setState({ status: 'error', message: e instanceof Error ? e.message : String(e) });
      });
    return () => {
      alive = false;
    };
  }, [runId, path, line, host]);

  if (state.status === 'loading') {
    return <div className="text-note text-ink-mute">Loading source…</div>;
  }

  if (state.status === 'error') {
    return <div className="text-note text-err">Could not load source: {state.message}</div>;
  }

  const { slice } = state;
  if (!slice.available) {
    return <div className="text-note text-ink-mute">{slice.reason ?? 'Source not available.'}</div>;
  }

  const language = slice.language && SOURCE_PREVIEW_LANGUAGES.has(slice.language) ? slice.language : null;
  const lines = slice.lines ?? [];

  return (
    <div className="mt-1 overflow-x-auto rounded-md border border-border bg-panel text-[11.5px]">
      <pre className="font-mono leading-5 px-0 py-0 m-0">
        {lines.map((ln) => {
          const isTarget = ln.n === slice.targetLine;
          return (
            <div
              key={ln.n}
              className={`flex ${isTarget ? 'bg-warn-bg' : ''}`}
              data-target={isTarget ? 'true' : undefined}
            >
              <span
                className="select-none pr-3 pl-3 text-right text-ink-mute"
                style={{ minWidth: '4ch' }}
              >
                {ln.n}
              </span>
              {language ? (
                <CodeHighlight code={ln.text} language={language as 'rust' | 'python' | 'typescript' | 'javascript' | 'go' | 'json'} inline />
              ) : (
                <code className="whitespace-pre font-mono text-ink">{ln.text}</code>
              )}
            </div>
          );
        })}
      </pre>
    </div>
  );
}
