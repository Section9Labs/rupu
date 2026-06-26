// Filter/control bar shared by the coverage concern tabs: severity dropdown,
// optional file text-filter, and expand/collapse-all.
const SEVERITIES = ['all', 'critical', 'high', 'medium', 'low', 'info'];

export default function ConcernControls({
  severity,
  onSeverity,
  fileQuery,
  onFileQuery,
  onExpandAll,
  onCollapseAll,
  total,
}: {
  severity: string;
  onSeverity: (s: string) => void;
  fileQuery?: string;
  onFileQuery?: (s: string) => void;
  onExpandAll: () => void;
  onCollapseAll: () => void;
  total: number;
}) {
  return (
    <div className="mb-3 flex flex-wrap items-center gap-2">
      <span className="text-[11px] text-ink-mute tabular-nums">{total} concerns</span>
      <select
        value={severity}
        onChange={(e) => onSeverity(e.target.value)}
        className="rounded-md border border-border bg-panel px-2 py-1 text-xs text-ink"
      >
        {SEVERITIES.map((s) => (
          <option key={s} value={s}>
            {s === 'all' ? 'all severities' : s}
          </option>
        ))}
      </select>
      {onFileQuery && (
        <input
          value={fileQuery ?? ''}
          onChange={(e) => onFileQuery(e.target.value)}
          placeholder="filter files…"
          className="rounded-md border border-border bg-panel px-2 py-1 text-xs text-ink"
        />
      )}
      <div className="ml-auto flex gap-1">
        <button
          onClick={onExpandAll}
          className="rounded-md border border-border px-2 py-1 text-xs text-ink-dim hover:bg-slate-100"
        >
          expand all
        </button>
        <button
          onClick={onCollapseAll}
          className="rounded-md border border-border px-2 py-1 text-xs text-ink-dim hover:bg-slate-100"
        >
          collapse all
        </button>
      </div>
    </div>
  );
}
