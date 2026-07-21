/**
 * CodeViewer — whole-file source view with inline PR-style finding cards and
 * severity line-bands. The centerpiece of the findings-on-code Code tab.
 *
 * Fetches the file via `api.getProjectSource` on mount (and whenever
 * `wsId`/`path` change), then renders it line-numbered with:
 *   - a severity-tinted background band + left `border-l-2` on any line that
 *     anchors one or more findings (worst severity wins the band colour when
 *     several findings share a line — see `byLine`/`severityRank`);
 *   - a squiggle underline on the anchored code text (`.finding-squiggle`,
 *     `codeViewer.css`) as a secondary "look here" cue independent of the
 *     band, matching the aikido reference;
 *   - one collapsed `InlineFindingCard` per finding directly under the line,
 *     stacking when several findings share an anchor.
 *
 * States: loading / error / unavailable (`file.available === false`, shows
 * the backend's `reason`) / loaded.
 */

import { Fragment, useEffect, useMemo, useRef, useState, type CSSProperties } from 'react';
import { Loader2 } from 'lucide-react';
import { api, type FileContent, type FindingRecord } from '../../lib/api';
import CodeHighlight, { HIGHLIGHTABLE_LANGUAGES, type Language } from '../CodeHighlight';
import { SEVERITY_STYLE, severityRank, type Severity } from '../../lib/severity';
import { isFindingStale } from './drift';
import InlineFindingCard from './InlineFindingCard';
import './codeViewer.css';

export interface CodeViewerProps {
  wsId: string;
  path: string;
  findings: FindingRecord[];
  /** Line to scroll into view + emphasize once the file loads (e.g. the line
   *  a finding-list row was clicked from). */
  initialLine?: number;
}

type Load =
  | { state: 'loading' }
  | { state: 'error'; msg: string }
  | { state: 'loaded'; file: FileContent };

/** Group findings by their anchor line (`line_range[0]`); within a line,
 *  worst severity first so the band + stack order reflect the most severe
 *  finding. Findings without a `line_range` can't be anchored and are
 *  dropped (they still surface in the file/project-level findings lists). */
function byLine(findings: FindingRecord[]): Map<number, FindingRecord[]> {
  const grouped = new Map<number, FindingRecord[]>();
  for (const f of findings) {
    if (!f.line_range) continue;
    const anchor = f.line_range[0];
    const arr = grouped.get(anchor) ?? [];
    arr.push(f);
    grouped.set(anchor, arr);
  }
  for (const arr of grouped.values()) {
    arr.sort((a, b) => severityRank(b.severity as Severity) - severityRank(a.severity as Severity));
  }
  return grouped;
}

export default function CodeViewer({ wsId, path, findings, initialLine }: CodeViewerProps) {
  const [load, setLoad] = useState<Load>({ state: 'loading' });
  const anchorRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    let live = true;
    setLoad({ state: 'loading' });
    api
      .getProjectSource(wsId, path)
      .then((file) => {
        if (live) setLoad({ state: 'loaded', file });
      })
      .catch((e) => {
        if (live) setLoad({ state: 'error', msg: e instanceof Error ? e.message : String(e) });
      });
    return () => {
      live = false;
    };
  }, [wsId, path]);

  const findingsForFile = useMemo(
    () => findings.filter((f) => f.file_path === path),
    [findings, path],
  );
  const grouped = useMemo(() => byLine(findingsForFile), [findingsForFile]);

  // Scroll the requested line into view once the file has loaded.
  useEffect(() => {
    if (load.state === 'loaded' && initialLine && anchorRef.current) {
      anchorRef.current.scrollIntoView({ block: 'center' });
    }
  }, [load.state, initialLine]);

  if (load.state === 'loading') {
    return (
      <div className="flex h-40 items-center justify-center gap-2 text-sm text-ink-dim">
        <Loader2 size={14} className="animate-spin" /> Loading source…
      </div>
    );
  }

  if (load.state === 'error') {
    return <div className="p-4 text-sm text-err">Could not load file: {load.msg}</div>;
  }

  const { file } = load;
  if (!file.available || !file.lines) {
    return (
      <div className="p-6 text-sm text-ink-dim">{file.reason ?? 'This file cannot be displayed.'}</div>
    );
  }

  const lang =
    file.language && HIGHLIGHTABLE_LANGUAGES.has(file.language) ? (file.language as Language) : null;

  return (
    <div className="h-full overflow-auto rounded-md border border-border bg-panel text-[12px]">
      <div className="font-mono">
        {file.lines.map((ln) => {
          const here = grouped.get(ln.n);
          const worst = here?.[0];
          const sev = worst ? ((worst.severity as Severity) ?? 'info') : null;
          const style = worst ? (SEVERITY_STYLE[worst.severity as Severity] ?? SEVERITY_STYLE.info) : null;
          const isAnchorLine = initialLine === ln.n;

          return (
            <Fragment key={ln.n}>
              <div
                ref={isAnchorLine ? anchorRef : undefined}
                data-line-row={ln.n}
                data-finding-line={here ? ln.n : undefined}
                className={`grid h-5 grid-cols-[4ch_1fr] leading-5 ${
                  style ? `${style.bg} border-l-2 ${style.barBorder}` : 'border-l-2 border-transparent'
                }`}
              >
                <span className="h-5 select-none pl-3 pr-3 text-right leading-5 text-ink-mute">
                  {ln.n}
                </span>
                <span
                  className={`h-5 min-w-0 leading-5 ${here ? 'finding-squiggle' : ''}`}
                  style={sev ? ({ '--squiggle-rgb': `var(--c-sev-${sev})` } as CSSProperties) : undefined}
                >
                  {lang ? (
                    <CodeHighlight code={ln.text} language={lang} inline />
                  ) : (
                    <code className="whitespace-pre font-mono text-ink">{ln.text}</code>
                  )}
                </span>
              </div>
              {here && (
                <div className="pl-[4ch] pr-3">
                  {here.map((f) => (
                    <InlineFindingCard
                      key={f.id}
                      finding={f}
                      stale={isFindingStale(f.evidence?.code_excerpt, file.lines!, f.line_range)}
                    />
                  ))}
                </div>
              )}
            </Fragment>
          );
        })}
      </div>
    </div>
  );
}
