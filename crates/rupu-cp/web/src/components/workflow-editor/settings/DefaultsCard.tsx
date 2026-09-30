// DefaultsCard — the workflow-level `defaults:` card for the Settings
// inspector. Only `findings_profile` is authored here: the profile a step's
// agent (or `action: findings.record` call) records under when the step sets
// none of its own. Every change reads the CURRENT model fresh off `rest` and
// writes back through `writeDefaults`, so sibling `defaults` keys
// (`continue_on_error`, `workspace`, …) survive untouched and are listed
// read-only below the select. Unset omits the key (and `defaults` itself when
// it ends up empty).

import { readDefaults, writeDefaults, type FindingsProfile } from '../../../lib/workflowMeta';

interface DefaultsCardProps {
  rest: Record<string, unknown>;
  onRest: (rest: Record<string, unknown>) => void;
}

const fieldCls =
  'w-full rounded-md border border-border bg-panel px-2.5 py-1.5 text-lead text-ink placeholder:text-ink-mute focus:border-brand-500 focus:outline-none';
const labelCls = 'mb-1 block text-ui font-semibold uppercase tracking-wide text-ink-dim';

/** Keys under `defaults:` this card edits itself (not listed as "other"). */
const EDITED_KEYS = new Set(['findings_profile']);

function otherDefaultsKeys(rest: Record<string, unknown>): string[] {
  const raw = rest.defaults;
  if (typeof raw !== 'object' || raw === null || Array.isArray(raw)) return [];
  return Object.keys(raw).filter((k) => !EDITED_KEYS.has(k));
}

export default function DefaultsCard({ rest, onRest }: DefaultsCardProps) {
  const model = readDefaults(rest);
  const others = otherDefaultsKeys(rest);

  function setProfile(value: string): void {
    const findings_profile: FindingsProfile | undefined =
      value === 'full' || value === 'summary' ? value : undefined;
    onRest(writeDefaults(rest, { ...model, findings_profile }));
  }

  return (
    <div className="wfx-card" data-testid="defaults-card">
      <div className="wfx-card-h">Defaults</div>
      <div className="wfx-card-b">
        <label className="block">
          <span className={labelCls}>Default findings profile</span>
          <select
            value={model.findings_profile ?? ''}
            onChange={(e) => setProfile(e.target.value)}
            aria-label="Default findings profile"
            className={fieldCls}
          >
            <option value="">Agent decides</option>
            <option value="full">Full report</option>
            <option value="summary">Summary</option>
          </select>
          <span className="mt-1 block text-note text-ink-mute">
            Applies to local steps that don't set their own. Not supported with remote (host:) steps.
          </span>
        </label>

        {others.length > 0 && (
          <p className="text-note text-ink-mute" data-testid="defaults-other-keys">
            Other defaults (edit in YAML): <span className="font-mono">{others.join(', ')}</span>
          </p>
        )}
      </div>
    </div>
  );
}
