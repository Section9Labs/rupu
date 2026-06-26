// Diff tab — compare two runs' contributions to a target. Base/compare pickers
// default to previous vs latest (the CLI default).
import { useEffect, useState } from 'react';
import { api, type RunListEntry, type RunDiff } from '../../lib/api';
import { SectionHeader } from '../lists/SectionHeader';
import { ListCard } from '../lists/ListCard';

export default function CoverageDiffTab({ target, wsId }: { target: string; wsId?: string }) {
  const [runs, setRuns] = useState<RunListEntry[] | null>(null);
  const [base, setBase] = useState('previous');
  const [compare, setCompare] = useState('latest');
  const [diff, setDiff] = useState<RunDiff | null>(null);
  const [error, setError] = useState<string | null>(null);

  // Load the run list once (for the pickers + the "need 2 runs" guard).
  useEffect(() => {
    let cancelled = false;
    api
      .getCoverageRuns(target, wsId)
      .then((r) => {
        if (!cancelled) setRuns(r);
      })
      .catch((e: unknown) => {
        if (!cancelled) setError(e instanceof Error ? e.message : 'Failed to load runs');
      });
    return () => {
      cancelled = true;
    };
  }, [target, wsId]);

  // Recompute the diff whenever the selectors change (and there are ≥2 runs).
  useEffect(() => {
    if (!runs || runs.length < 2) return;
    let cancelled = false;
    setDiff(null);
    setError(null);
    api
      .getCoverageDiff(target, { wsId, base, compare })
      .then((d) => {
        if (!cancelled) setDiff(d);
      })
      .catch((e: unknown) => {
        if (!cancelled) setError(e instanceof Error ? e.message : 'Failed to load diff');
      });
    return () => {
      cancelled = true;
    };
  }, [target, wsId, base, compare, runs]);

  if (error) return <p className="mt-4 text-sm text-red-700">{error}</p>;
  if (!runs) return <p className="mt-4 text-sm text-ink-dim">Loading…</p>;
  if (runs.length < 2)
    return (
      <p className="mt-4 text-sm text-ink-dim">
        Need at least two runs on this target to diff (found {runs.length}).
      </p>
    );

  const options = [
    { value: 'previous', label: 'previous' },
    { value: 'latest', label: 'latest' },
    ...runs.map((r) => ({ value: r.run_id, label: `${r.run_id} (${r.model})` })),
  ];

  return (
    <div className="mt-6 space-y-6">
      <div className="flex items-end gap-3">
        <Picker label="Base" value={base} onChange={setBase} options={options} />
        <span className="pb-1.5 text-ink-mute">→</span>
        <Picker label="Compare" value={compare} onChange={setCompare} options={options} />
      </div>

      {!diff ? (
        <p className="text-sm text-ink-dim">Computing diff…</p>
      ) : (
        <>
          <CellSection title="Newly asserted" tone="good" cells={diff.newly_asserted} />
          <FlipSection flips={diff.verdict_flips} />
          <CellSection title="No longer asserted" tone="muted" cells={diff.no_longer_asserted} />
          <ThemeSection title="Findings appeared" tone="bad" themes={diff.findings_appeared} />
          <ThemeSection
            title="Findings disappeared"
            tone="muted"
            themes={diff.findings_disappeared}
          />
          <FileSection title="Newly touched" files={diff.newly_touched} />
          <FileSection title="No longer touched" files={diff.no_longer_touched} />
        </>
      )}
    </div>
  );
}

function Picker({
  label,
  value,
  onChange,
  options,
}: {
  label: string;
  value: string;
  onChange: (v: string) => void;
  options: { value: string; label: string }[];
}) {
  return (
    <label className="flex flex-col gap-1">
      <span className="text-[11px] text-ink-mute">{label}</span>
      <select
        value={value}
        onChange={(e) => onChange(e.target.value)}
        className="rounded-md border border-border bg-panel px-2 py-1 text-sm text-ink"
      >
        {options.map((o) => (
          <option key={o.value} value={o.value}>
            {o.label}
          </option>
        ))}
      </select>
    </label>
  );
}

function CellSection({
  title,
  tone,
  cells,
}: {
  title: string;
  tone: 'good' | 'muted';
  cells: { concern_id: string; file_path: string; status: string }[];
}) {
  if (cells.length === 0) return null;
  return (
    <section>
      <SectionHeader tone={tone} label={title} count={cells.length} />
      <ListCard>
        {cells.map((c, i) => (
          <div key={`${c.concern_id}:${c.file_path}:${i}`} className="px-4 py-2 text-xs">
            <span className="font-mono text-ink">{c.concern_id}</span>
            <span className="text-ink-mute"> · {c.file_path}</span>
            <span className="ml-2 text-ink-mute">{c.status}</span>
          </div>
        ))}
      </ListCard>
    </section>
  );
}

function FlipSection({
  flips,
}: {
  flips: {
    concern_id: string;
    file_path: string;
    base_status: string;
    compare_status: string;
    high_signal: boolean;
  }[];
}) {
  if (flips.length === 0) return null;
  return (
    <section>
      <SectionHeader
        tone="bad"
        label="Verdict flips"
        count={flips.length}
        hint="clean→finding highlighted"
      />
      <ListCard>
        {flips.map((f, i) => (
          <div key={`${f.concern_id}:${f.file_path}:${i}`} className="px-4 py-2 text-xs">
            <span className="font-mono text-ink">{f.concern_id}</span>
            <span className="text-ink-mute"> · {f.file_path}</span>
            <span
              className={f.high_signal ? 'ml-2 font-medium text-red-700' : 'ml-2 text-ink-mute'}
            >
              {f.base_status} → {f.compare_status}
            </span>
          </div>
        ))}
      </ListCard>
    </section>
  );
}

function ThemeSection({
  title,
  tone,
  themes,
}: {
  title: string;
  tone: 'bad' | 'muted';
  themes: { concern_id: string | null; theme: string }[];
}) {
  if (themes.length === 0) return null;
  return (
    <section>
      <SectionHeader tone={tone} label={title} count={themes.length} />
      <ListCard>
        {themes.map((t, i) => (
          <div key={`${t.theme}:${i}`} className="px-4 py-2 text-xs">
            <span className="text-ink">{t.theme}</span>
            {t.concern_id && <span className="ml-2 font-mono text-ink-mute">{t.concern_id}</span>}
          </div>
        ))}
      </ListCard>
    </section>
  );
}

function FileSection({ title, files }: { title: string; files: string[] }) {
  if (files.length === 0) return null;
  return (
    <section>
      <SectionHeader tone="muted" label={title} count={files.length} />
      <ListCard>
        {files.map((f) => (
          <div key={f} className="px-4 py-2 text-[11px] font-mono text-ink-mute break-all">
            {f}
          </div>
        ))}
      </ListCard>
    </section>
  );
}
