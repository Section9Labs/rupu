// Live Events — the project roster beside the stream. One row per project,
// ordered awaiting → running → idle (see buildRoster): its name, a live chip,
// what it is doing now or its branch, its active runs and its findings by
// severity. All real; a row links to the project.

import { Link } from 'react-router-dom';
import type { RosterProject, SevCounts } from '../../lib/situationRoom/roster';
import { Rolling } from '../events/parts';

function SevChips({ f }: { f: SevCounts }) {
  if (f.total === 0) return <span className="text-ink-mute">no findings</span>;
  return (
    <span className="evx-sevs">
      {(['critical', 'high', 'medium', 'low', 'info'] as const).map((s) =>
        f[s] > 0 ? (
          <span key={s} className="evx-sevchip evx-m" data-mark={s} title={`${f[s]} ${s}`}>
            <i aria-hidden />
            <b>
              <Rolling value={f[s]} />
            </b>
          </span>
        ) : null,
      )}
    </span>
  );
}

const WORD: Record<RosterProject['status'], string> = { await: 'needs you', running: 'live', idle: 'idle' };

export default function ProjectRoster({ roster }: { roster: RosterProject[] }) {
  const live = roster.filter((r) => r.status !== 'idle').length;
  return (
    <aside className="hidden min-h-0 w-[300px] shrink-0 flex-col border-l border-border bg-panel lg:flex">
      <div className="flex h-10 items-center gap-2 border-b border-border px-4">
        <h2 className="m-0 text-meta font-semibold uppercase tracking-[0.08em] text-ink-mute">Projects</h2>
        <span className="ml-auto font-mono text-note tabular-nums text-ink-mute">
          {roster.length} · <Rolling value={live} /> live
        </span>
      </div>
      <div className="min-h-0 overflow-auto py-1.5">
        {roster.length === 0 ? (
          <div className="p-6 text-center text-note text-ink-dim">No projects yet.</div>
        ) : (
          roster.map((p) => (
            <Link key={p.wsId} data-testid="sr-pcard" to={`/projects/${p.wsId}`} className="evx-proj" data-idle={p.status === 'idle' ? '' : undefined}>
              <span className="evx-pn">{p.name}</span>
              <span className="evx-livechip" data-state={p.status}>
                <i aria-hidden />
                {WORD[p.status]}
              </span>
              <span className="evx-pr">
                {p.action ? (
                  <span className="evx-act">{p.action}</span>
                ) : (
                  p.branch && <span className="evx-act font-mono">{p.branch}</span>
                )}
                {p.activeRuns > 0 && <span className="tabular-nums">{p.activeRuns} active</span>}
                <SevChips f={p.findings} />
              </span>
            </Link>
          ))
        )}
      </div>
    </aside>
  );
}
