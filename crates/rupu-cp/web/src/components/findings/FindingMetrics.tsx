// Metric-tile strip for findings: Total · Critical · High · Medium · Low · Info.
// Each severity tile carries its `SEVERITY_STYLE` accent; the Total tile is
// neutral. When `onSelect` is supplied the tiles become filter buttons
// (severity → onSelect(sev), Total → onSelect(null)) with the `active` tile
// ring-highlighted. Static (non-interactive) otherwise.

import { type FindingsSummary } from '../../lib/api';
import { cn } from '../../lib/cn';
import { SEVERITY_STYLE, type Severity } from '../../lib/severity';

export interface FindingMetricsProps {
  summary: FindingsSummary;
  /** Currently-active severity filter, or null for Total. Only meaningful
   *  when `onSelect` is provided. */
  active?: Severity | null;
  /** When provided, tiles become buttons; clicking fires this with the
   *  chosen severity (or null for the Total tile). */
  onSelect?: (sev: Severity | null) => void;
}

// Severity tiles in display order. Total is rendered separately (neutral).
const SEV_TILES: Severity[] = ['critical', 'high', 'medium', 'low', 'info'];

export function FindingMetrics({ summary, active = null, onSelect }: FindingMetricsProps) {
  const interactive = Boolean(onSelect);

  return (
    <div className="grid grid-cols-3 sm:grid-cols-6 gap-2">
      <Tile
        label="Total"
        value={summary.total}
        valueClass="text-ink"
        interactive={interactive}
        active={interactive && active === null}
        onClick={onSelect ? () => onSelect(null) : undefined}
      />
      {SEV_TILES.map((sev) => {
        const s = SEVERITY_STYLE[sev];
        return (
          <Tile
            key={sev}
            label={s.label}
            value={summary[sev]}
            valueClass={s.text}
            interactive={interactive}
            active={interactive && active === sev}
            onClick={onSelect ? () => onSelect(sev) : undefined}
          />
        );
      })}
    </div>
  );
}

interface TileProps {
  label: string;
  value: number;
  /** STATIC text-colour class for the value (from SEVERITY_STYLE / neutral). */
  valueClass: string;
  interactive: boolean;
  active: boolean;
  onClick?: () => void;
}

function Tile({ label, value, valueClass, interactive, active, onClick }: TileProps) {
  const body = (
    <>
      <div className="text-note uppercase tracking-wide text-ink-mute font-medium">{label}</div>
      <div className={cn('text-2xl font-semibold mt-0.5 tabular-nums', valueClass)}>{value}</div>
    </>
  );

  const base = 'bg-panel border border-border rounded-xl px-4 py-3 text-left';

  if (interactive && onClick) {
    return (
      <button
        type="button"
        onClick={onClick}
        aria-pressed={active}
        aria-label={`Filter by ${label}`}
        className={cn(
          base,
          'transition-shadow hover:border-border',
          active && 'ring-2 ring-brand-400 border-brand-300',
        )}
      >
        {body}
      </button>
    );
  }

  return <div className={base}>{body}</div>;
}
