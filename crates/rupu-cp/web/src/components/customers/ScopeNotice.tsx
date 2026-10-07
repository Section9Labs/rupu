// One-line, dismissible info line for `useCustomerScope().notice` (shown when
// a stored scope was rejected and cleared). Mounted at the top of each shell's
// main content area.

import { useState } from 'react';
import { Info, X } from 'lucide-react';
import { useCustomerScope } from '../../lib/customerScope';

export function ScopeNotice() {
  const { notice, noticeSeq } = useCustomerScope();
  // Keyed by occurrence, so a repeated identical notice shows again.
  const [dismissed, setDismissed] = useState<number | null>(null);
  if (!notice || dismissed === noticeSeq) return null;
  return (
    <div
      role="status"
      className="flex items-center gap-2 border-b border-border bg-brand-50 px-4 py-1.5 text-[12px] text-brand-700"
    >
      <Info size={13} aria-hidden className="shrink-0" />
      <span className="flex-1">{notice}</span>
      <button
        type="button"
        aria-label="Dismiss notice"
        onClick={() => setDismissed(noticeSeq)}
        className="rounded p-0.5 hover:bg-surface-hover"
      >
        <X size={13} aria-hidden />
      </button>
    </div>
  );
}
