// UnpricedBanner — the spend we cannot account for, stated plainly.
//
// This was a '*' footnote. On an attribution page that is not good enough: if
// some models have no price, the headline number is an UNDER-COUNT, and a
// number that is quietly wrong is worse than no number.

import type { UnpricedGap } from '../../lib/usage';

export type { UnpricedGap };

export function UnpricedBanner({ unpriced }: { unpriced: UnpricedGap }) {
  if (unpriced.models.length === 0) return null;
  return (
    <div className="rounded-lg border border-[rgb(var(--c-status-awaiting))] bg-[rgb(var(--c-surface))] px-4 py-2 text-sm">
      <span className="font-medium text-[rgb(var(--c-ink))]">
        {unpriced.models.length} model{unpriced.models.length === 1 ? '' : 's'} unpriced
      </span>
      <span className="text-[rgb(var(--c-ink-dim))]">
        {' '}
        — spend below excludes {unpriced.rows} token row
        {unpriced.rows === 1 ? '' : 's'} from {unpriced.models.join(', ')}
      </span>
    </div>
  );
}
