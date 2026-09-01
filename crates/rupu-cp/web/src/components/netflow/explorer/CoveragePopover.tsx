// "Coverage & gaps" — the v3 redesign's "honesty on demand" affordance:
// the scope disclosure, the fidelity legend, and the dropped-loss
// accounting move out of the always-visible page body into ONE consistent,
// loud-enough (amber) pill + popover, reachable at every scope.
//
// Nothing here authors copy: the disclosure comes from
// `ScopeDisclosure.tsx`'s `disclosureText` (still the single source of
// every scope-limit sentence, including run scope's sub-agent note), the
// legend from `FidelityBadge.tsx`'s `FIDELITY_TITLE`, and the dropped
// sentence from `NetflowTable.tsx`'s `droppedTotalSentence` (the same
// string the all-dropped empty-state banner renders).

import { useEffect, useRef, useState } from 'react';
import type { Fidelity } from '../../../lib/netflow';
import { FidelityBadge, FIDELITY_TITLE } from '../FidelityBadge';
import { droppedTotalSentence } from '../NetflowTable';
import { disclosureText, type NetflowScope } from '../ScopeDisclosure';

const LEGEND_ORDER: Fidelity[] = ['http', 'coarse', 'full'];

export function CoveragePopover({
  scope,
  droppedTotal,
}: {
  scope: NetflowScope;
  droppedTotal: number;
}) {
  const [open, setOpen] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);

  // Dismiss on outside click / Escape — standard popover contract.
  useEffect(() => {
    if (!open) return;
    const onPointerDown = (e: PointerEvent) => {
      if (rootRef.current && !rootRef.current.contains(e.target as Node)) setOpen(false);
    };
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === 'Escape') setOpen(false);
    };
    document.addEventListener('pointerdown', onPointerDown);
    document.addEventListener('keydown', onKeyDown);
    return () => {
      document.removeEventListener('pointerdown', onPointerDown);
      document.removeEventListener('keydown', onKeyDown);
    };
  }, [open]);

  return (
    <div ref={rootRef} className="relative flex-none">
      <button
        type="button"
        aria-expanded={open}
        onClick={() => setOpen((v) => !v)}
        className="inline-flex items-center gap-1.5 rounded-full border border-warn/35 bg-warn-bg px-2.5 py-1 text-note font-medium text-warn"
      >
        <span aria-hidden className="font-bold">
          ⓘ
        </span>
        Coverage &amp; gaps
        {/* Loss is stated ON the always-visible pill, not only inside the
            popover — a pill that looks identical at 0 and 40,000 dropped
            flows would be the silent-incompleteness defect the old
            always-on banner existed to prevent. */}
        {droppedTotal > 0 && (
          <span className="font-semibold">· {droppedTotal} dropped</span>
        )}
      </button>
      {open && (
        <div
          role="dialog"
          aria-label="What this view covers"
          className="absolute right-0 top-9 z-50 w-[26rem] max-w-[90vw] rounded-xl border border-border bg-panel p-4 shadow-lg"
        >
          <p className="mb-2 text-meta font-semibold uppercase tracking-wider text-ink-mute">
            What this view covers
          </p>
          <p className="mb-3 text-note leading-relaxed text-ink-dim">{disclosureText(scope)}</p>
          <div className="flex flex-col gap-2 border-t border-border pt-2.5">
            {LEGEND_ORDER.map((f) => (
              <div key={f} className="flex items-baseline gap-2">
                <span className="flex-none">
                  <FidelityBadge fidelity={f} />
                </span>
                <span className="text-note text-ink-dim">{FIDELITY_TITLE[f]}</span>
              </div>
            ))}
            {droppedTotal > 0 && (
              <div className="flex items-baseline gap-2">
                <span className="flex-none rounded bg-warn-bg px-1.5 py-0.5 text-meta font-medium text-warn">
                  {droppedTotal} dropped
                </span>
                <span className="text-note text-ink-dim">
                  {droppedTotalSentence(droppedTotal)}
                </span>
              </div>
            )}
          </div>
        </div>
      )}
    </div>
  );
}

export default CoveragePopover;
