import type { UsageSummary } from '../lib/usage';
import { formatTokens, formatCost } from '../lib/usage';

/**
 * Compact inline usage chip: `· 4,210 tok · $0.03`.
 * - Cost shows `—` when unpriced (`cost_usd === null`).
 * - A partial cost (some models unpriced, `priced === false` but a cost exists)
 *   is suffixed with `*` and a hover title.
 */
export default function UsageChip({
  usage,
  className = '',
}: {
  usage: UsageSummary;
  className?: string;
}) {
  const partial = usage.cost_usd !== null && !usage.priced;
  const costTitle = partial
    ? 'Partial — some models have no price configured'
    : undefined;
  return (
    <span className={`inline-flex items-center gap-1.5 text-note text-ink-mute tabular-nums ${className}`}>
      <span>{formatTokens(usage.total_tokens)} tok</span>
      <span className="text-border">·</span>
      <span title={costTitle} className={partial ? 'text-amber-600' : undefined}>
        {formatCost(usage.cost_usd)}{partial ? '*' : ''}
      </span>
    </span>
  );
}
