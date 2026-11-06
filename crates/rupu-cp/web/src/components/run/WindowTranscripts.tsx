// The Transcript tab while a turn window is on: every transcript the window's
// turns came from (a step's, or one unit's of a fan-out), each with the range
// of its model calls inside the window; the chosen one shows only those turns.

import { useEffect, useMemo, useState } from 'react';
import TranscriptPanel from '../TranscriptPanel';
import type { RunGraphModel } from '../../lib/runGraphModel';
import type { TurnWindow } from '../../lib/run/turnWindow';
import { parseCodename } from '../../lib/codename';
import { cn } from '../../lib/cn';

/** What to call a transcript: the step it belongs to, or `step · unit`. */
export function transcriptLabel(model: RunGraphModel | null, path: string): string {
  for (const n of model?.nodes ?? []) {
    if (n.transcriptPath === path) return n.id;
    const unit = n.fanout?.units.find((u) => u.transcriptPath === path);
    if (unit) return `${n.id} · ${unit.codename ? parseCodename(unit.codename).leaf : unit.key}`;
  }
  const base = path.split('/').pop() ?? path;
  return base.replace(/\.jsonl$/, '');
}

export default function WindowTranscripts({
  window: w,
  model,
  preferred,
  live,
  runId,
  host,
}: {
  window: TurnWindow;
  model: RunGraphModel | null;
  /** The transcript the page had selected, shown first when it is in the window. */
  preferred: string | null;
  live: boolean;
  runId: string;
  host?: string;
}) {
  const entries = useMemo(() => [...w.transcripts], [w]);
  const [chosen, setChosen] = useState<string | null>(null);
  // Pick again only when the choice is no longer in the window (a live run
  // recomputes the window on every poll): the page's own transcript if the
  // window has it, else the first one the window meets.
  useEffect(() => {
    setChosen((cur) =>
      cur && w.transcripts.has(cur)
        ? cur
        : preferred && w.transcripts.has(preferred)
          ? preferred
          : (entries[0]?.[0] ?? null),
    );
  }, [w, preferred, entries]);

  if (entries.length === 0) {
    return (
      <div className="flex h-full min-h-[120px] items-center justify-center rounded-xl border border-border bg-panel px-6 text-center text-sm text-ink-dim">
        This control plane doesn&apos;t say which transcript each turn is in, so the transcript can&apos;t follow the
        window. Clear it to read the step&apos;s whole transcript.
      </div>
    );
  }
  const range = chosen ? w.transcripts.get(chosen) : undefined;
  return (
    <div className="flex h-full min-h-0 flex-col gap-2">
      <div className="flex flex-wrap gap-1.5" role="tablist" aria-label="Transcripts in the window">
        {entries.map(([path, [a, b]]) => (
          <button
            key={path}
            type="button"
            role="tab"
            aria-selected={path === chosen}
            onClick={() => setChosen(path)}
            title={path}
            className={cn(
              'inline-flex max-w-[22rem] items-center gap-1.5 rounded-md border px-2 py-1 text-note transition-colors',
              path === chosen
                ? 'border-brand-500/50 bg-brand-500/10 text-ink'
                : 'border-border bg-panel text-ink-dim hover:bg-surface-hover',
            )}
          >
            <span className="truncate font-mono">{transcriptLabel(model, path)}</span>
            <span className="shrink-0 tabular-nums text-ink-mute">
              {a === b ? `call ${a}` : `calls ${a}–${b}`}
            </span>
          </button>
        ))}
      </div>
      <div className="min-h-0 flex-1 overflow-auto">
        {chosen && range && (
          <TranscriptPanel key={chosen} path={chosen} live={live} runId={runId} host={host} calls={range} />
        )}
      </div>
    </div>
  );
}
