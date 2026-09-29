// Situation Room — right-rail project roster. One compact card per project,
// ordered awaiting → running → idle (see buildRoster). Status dot, current
// live action, findings-by-severity pips, and active-run count — all real,
// no fabricated progress. Clicking a card deep-links to the project page.
//
// Card chrome mirrors the live-stream EventCards (Ghost's language): a
// bordered `bg-panel` card that lifts on hover, mono project name, meta-scale
// status, a themed status Dot.

import { Loader2 } from 'lucide-react';
import { Link } from 'react-router-dom';
import { Dot, type DotColor } from '../ui/Dot';
import type { RosterProject, SevCounts } from '../../lib/situationRoom/roster';

function Pips({ f }: { f: SevCounts }) {
  if (f.total === 0) return <span className="text-ink-mute">no findings</span>;
  return (
    <span className="inline-flex items-center gap-1.5 font-mono tabular-nums">
      {f.critical > 0 && <span className="sr-sev-critical">{f.critical}</span>}
      {f.high > 0 && <span className="sr-sev-high">{f.high}</span>}
      {f.medium > 0 && <span className="sr-sev-medium">{f.medium}</span>}
      {f.low > 0 && <span className="sr-sev-low">{f.low}</span>}
      {f.info > 0 && <span className="sr-sev-info">{f.info}</span>}
    </span>
  );
}

function stTxt(status: RosterProject['status']): string {
  return status === 'await' ? 'awaiting' : status;
}

function dotColor(status: RosterProject['status']): DotColor {
  return status === 'await' ? 'awaiting' : status === 'running' ? 'brand' : 'mute';
}

export default function ProjectRoster({ roster }: { roster: RosterProject[] }) {
  const live = roster.filter((r) => r.status !== 'idle').length;
  return (
    <aside className="flex w-[336px] shrink-0 flex-col min-h-0 border-l border-border bg-panel/50">
      <div className="flex items-center gap-2 border-b border-border px-4 py-3">
        <h2 className="m-0 text-ui font-semibold uppercase tracking-[0.14em] text-ink-dim">Projects</h2>
        <span className="ml-auto font-mono text-note tabular-nums text-ink-mute">
          {roster.length} · {live} live
        </span>
      </div>
      <div className="flex flex-col gap-2 overflow-auto p-2.5 min-h-0">
        {roster.length === 0 ? (
          <div className="p-6 text-center text-note text-ink-dim">No projects yet.</div>
        ) : (
          roster.map((p) => (
            <Link
              key={p.wsId}
              data-testid="sr-pcard"
              to={`/projects/${p.wsId}`}
              className="block rounded-lg border border-border bg-panel px-3 py-2 no-underline transition-colors hover:border-ink-mute"
            >
              <div className="flex items-center gap-2">
                <Dot color={dotColor(p.status)} />
                <span className="min-w-0 truncate font-mono text-note font-medium text-ink">{p.name}</span>
                <span className="ml-auto shrink-0 text-meta uppercase tracking-wide text-ink-mute">{stTxt(p.status)}</span>
              </div>
              {p.action ? (
                <div className="mt-1 flex items-center gap-1.5 text-note text-ink-dim">
                  {p.status === 'running' && <Loader2 className="h-3 w-3 shrink-0 animate-spin" />}
                  <span className="min-w-0 truncate">{p.action}</span>
                </div>
              ) : (
                p.branch && <div className="mt-1 truncate font-mono text-meta text-ink-mute">{p.branch}</div>
              )}
              <div className="mt-1.5 flex items-center gap-3 text-meta text-ink-mute">
                {p.activeRuns > 0 && <span className="tabular-nums">{p.activeRuns} active</span>}
                <Pips f={p.findings} />
              </div>
            </Link>
          ))
        )}
      </div>
    </aside>
  );
}
