// Field — a settings field in one rhythm (spec §"Form fields"): a label with
// its config-origin chip inline, an optional policy-lock note (an inline note,
// not a floating lock icon), one line of help, then the control (children).
//
// Rendering only; it owns the label/help/chip rhythm, not the control. Callers
// pass the input/select/toggle as children. Token classes only. (Shared
// component for the Settings migration — not used by the Dashboard.)

import type { ReactNode } from 'react';
import { Lock } from 'lucide-react';
import { StatusChip } from './StatusChip';

export interface FieldProps {
  label: ReactNode;
  /** Config origin — rendered as the inline origin chip. */
  origin?: 'global' | 'project';
  /** One line of help under the label. */
  help?: ReactNode;
  /** When set, shows an inline "locked by policy" note and (by convention) the
   *  caller renders the control disabled. */
  locked?: boolean;
  /** The control. */
  children?: ReactNode;
}

export function Field({ label, origin, help, locked = false, children }: FieldProps) {
  return (
    <div className="border-t border-border py-3.5 first:border-t-0">
      <div className="flex flex-wrap items-center gap-2">
        <span className="text-lead font-semibold text-ink">{label}</span>
        {origin && (
          <StatusChip tone={origin === 'global' ? 'origin-global' : 'origin-project'}>
            {origin}
          </StatusChip>
        )}
        {locked && (
          <span className="inline-flex items-center gap-1 rounded-full bg-surface px-2 py-0.5 text-note font-medium text-ink-dim ring-1 ring-border">
            <Lock size={11} aria-hidden />
            locked by policy
          </span>
        )}
      </div>
      {help != null && help !== '' && <p className="mt-0.5 text-note text-ink-mute">{help}</p>}
      {children && <div className="mt-2">{children}</div>}
    </div>
  );
}

export default Field;
