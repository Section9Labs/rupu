// Situation Room — right-rail project roster. One compact card per project,
// ordered awaiting → running → idle (see buildRoster). Status dot, current
// live action, findings-by-severity pips, and active-run count — all real,
// no fabricated progress. Clicking a card deep-links to the project page.

import { Loader2 } from 'lucide-react';
import { Link } from 'react-router-dom';
import { cn } from '../../lib/cn';
import type { RosterProject, SevCounts } from '../../lib/situationRoom/roster';

function Pips({ f }: { f: SevCounts }) {
  if (f.total === 0) return <span className="text-ink-mute">no findings</span>;
  return (
    <>
      {f.critical > 0 && <span className="sr-sev-critical">{f.critical}</span>}
      {f.high > 0 && <span className="sr-sev-high">{f.high}</span>}
      {f.medium > 0 && <span className="sr-sev-medium">{f.medium}</span>}
      {f.low > 0 && <span className="sr-sev-low">{f.low}</span>}
      {f.info > 0 && <span className="sr-sev-info">{f.info}</span>}
    </>
  );
}

function stTxt(status: RosterProject['status']): string {
  return status === 'await' ? 'awaiting' : status;
}

export default function ProjectRoster({ roster }: { roster: RosterProject[] }) {
  const live = roster.filter((r) => r.status !== 'idle').length;
  return (
    <aside className="border-l border-border bg-panel/50 flex flex-col min-h-0 w-[336px] shrink-0">
      <div className="flex items-center gap-2 px-4 py-3 border-b border-border">
        <h2 className="text-ui tracking-[0.14em] uppercase text-ink-dim font-semibold m-0">Projects</h2>
        <span className="ml-auto text-note text-ink-mute tabular-nums font-mono">
          {roster.length} · {live} live
        </span>
      </div>
      <div className="overflow-auto p-2.5 flex flex-col gap-2 min-h-0">
        {roster.length === 0 ? (
          <div className="p-6 text-center text-note text-ink-dim">No projects yet.</div>
        ) : (
          roster.map((p) => (
            <Link
              key={p.wsId}
              to={`/projects/${p.wsId}`}
              className={cn('sr-pcard block no-underline', p.status !== 'idle' && 'hot')}
            >
              <div className="flex items-center gap-2">
                <span className={cn('sr-sdot', p.status)} />
                <span className="sr-pcard-repo text-ink">{p.name}</span>
                <span className="sr-pcard-st">{stTxt(p.status)}</span>
              </div>
              {p.action ? (
                <div className="sr-pcard-cur">
                  {p.status === 'running' && <Loader2 className="w-3 h-3 shrink-0 animate-spin" />}
                  <span>{p.action}</span>
                </div>
              ) : (
                p.branch && <div className="sr-pcard-cur"><span>{p.branch}</span></div>
              )}
              <div className="sr-pfoot">
                {p.activeRuns > 0 && <span>{p.activeRuns} active</span>}
                <span className="fp"><Pips f={p.findings} /></span>
              </div>
            </Link>
          ))
        )}
      </div>
    </aside>
  );
}
