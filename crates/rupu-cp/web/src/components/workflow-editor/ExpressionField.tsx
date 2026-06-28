// ExpressionField — a controlled editor for ONE workflow expression field
// (prompt / when / for_each / panel subject+prompt / sub-step prompt).
//
// The heavy CodeMirror wiring (highlighting + context-aware autocomplete) lives
// in `ExpressionFieldImpl`, loaded via `React.lazy` so the `@codemirror/*`
// packages stay in their own async chunk — shared with CodeEditorImpl, never in
// the main entry. Until that chunk resolves (and as a graceful fallback) a plain
// controlled `<input>` / `<textarea>` with the same API + form-field look is
// shown, so editing always works.

import { lazy, Suspense, useContext } from 'react';
import type { ExprContext } from '../../lib/workflowExpressions';
import { ThemeContext, type Mode } from '../theme/ThemeProvider';

export interface ExpressionFieldProps {
  value: string;
  onChange: (v: string) => void;
  context: ExprContext;
  multiline?: boolean;
  ariaLabel?: string;
  placeholder?: string;
  /** Resolved theme mode — injected by the wrapper so the imperative CodeMirror
   *  view picks the matching token colors and reconfigures on toggle. */
  theme?: Mode;
}

const ExpressionFieldImpl = lazy(() => import('./ExpressionFieldImpl'));

// Mirror StepForm's field look (border / radius / size) so the editor reads as a
// normal form control. `focus-within` lights the brand border like the inputs.
const SHELL_CLASS =
  'w-full overflow-hidden rounded-md border border-border bg-panel text-lead text-ink ' +
  'focus-within:border-brand-500';

const FALLBACK_CLASS =
  'w-full resize-y bg-panel px-2.5 py-1.5 font-mono text-lead text-ink ' +
  'placeholder:text-ink-mute focus:outline-none';

function Fallback({ value, onChange, multiline, ariaLabel, placeholder }: ExpressionFieldProps) {
  if (multiline) {
    return (
      <textarea
        value={value}
        onChange={(e) => onChange(e.target.value)}
        aria-label={ariaLabel}
        placeholder={placeholder}
        spellCheck={false}
        rows={4}
        className={FALLBACK_CLASS}
      />
    );
  }
  return (
    <input
      type="text"
      value={value}
      onChange={(e) => onChange(e.target.value)}
      aria-label={ariaLabel}
      placeholder={placeholder}
      spellCheck={false}
      className={FALLBACK_CLASS}
    />
  );
}

export default function ExpressionField(props: ExpressionFieldProps) {
  // Provider-optional: fall back to undefined (the impl reads `data-theme`) so
  // the field renders in isolated tests without a ThemeProvider.
  const mode = useContext(ThemeContext)?.mode;
  return (
    <div className={SHELL_CLASS}>
      <Suspense fallback={<Fallback {...props} />}>
        <ExpressionFieldImpl {...props} theme={mode} />
      </Suspense>
    </div>
  );
}
