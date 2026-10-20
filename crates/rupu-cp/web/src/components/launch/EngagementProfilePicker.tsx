// EngagementProfilePicker — the launcher sheets' "Engagement profiles"
// section (Plan 3b mock §5): the chosen profiles as chips, an "add a
// profile…" select, and under the chips what each one brings (its bundle and
// asset kinds), so a profile can be picked without reading its TOML. The
// list is `GET /api/engagement-profiles` for the target's project. Collapsed
// until opened; disabled, with the reason, where the launch can't carry it.

import { useEffect, useMemo, useState } from 'react';
import { X } from 'lucide-react';
import { apiErrorMessage } from '../../lib/api';
import { launchApi, profileSummary, type EngagementProfiles } from '../../lib/api3b/launch';
import { Select } from '../ui/Select';

export default function EngagementProfilePicker({
  value,
  onChange,
  wsId,
  disabledReason,
  note,
  disabled = false,
}: {
  value: string[];
  onChange: (ids: string[]) => void;
  /** The target's project, whose `.rupu/profiles` overlay the global ones. */
  wsId?: string;
  /** Why the launch can't carry profiles; the section is disabled when set. */
  disabledReason: string | null;
  /** A line under the picker when it is enabled (e.g. a remote host decides). */
  note?: string | null;
  disabled?: boolean;
}) {
  const [listing, setListing] = useState<EngagementProfiles | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    setLoadError(null);
    launchApi
      .getEngagementProfiles(wsId)
      .then((l) => {
        if (!cancelled) setListing(l);
      })
      .catch((e: unknown) => {
        if (!cancelled) setLoadError(apiErrorMessage(e));
      });
    return () => {
      cancelled = true;
    };
  }, [wsId]);

  const byId = useMemo(() => new Map((listing?.profiles ?? []).map((p) => [p.id, p])), [listing]);
  const addable = (listing?.profiles ?? []).filter((p) => !value.includes(p.id));
  const off = disabled || disabledReason !== null;

  return (
    <details className="group" data-testid="launch-profiles">
      <summary className="cursor-pointer list-none text-ui font-semibold uppercase tracking-wide text-ink-dim hover:text-ink">
        <span className="group-open:hidden">▸ Engagement profiles</span>
        <span className="hidden group-open:inline">▾ Engagement profiles</span>
        {value.length > 0 && (
          <span className="ml-1 normal-case tracking-normal text-ink-mute">({value.length})</span>
        )}
      </summary>
      <div className="mt-2 space-y-1.5">
        <div className="flex flex-wrap items-center gap-1.5 rounded-md border border-border bg-panel px-2 py-1.5">
          {value.map((id) => (
            <span
              key={id}
              className="inline-flex items-center gap-1 rounded-full bg-brand-50 px-2 py-0.5 text-note font-semibold text-brand-700 ring-1 ring-brand-100"
            >
              {id}
              <button
                type="button"
                onClick={() => onChange(value.filter((v) => v !== id))}
                disabled={off}
                aria-label={`Remove ${id}`}
                className="rounded-full text-brand-700/70 hover:text-brand-700 disabled:cursor-not-allowed"
              >
                <X size={11} aria-hidden />
              </button>
            </span>
          ))}
          <Select
            value=""
            onChange={(e) => {
              if (e.target.value) onChange([...value, e.target.value]);
            }}
            disabled={off || addable.length === 0}
            aria-label="Add an engagement profile"
            className="h-7 min-w-0 flex-1 border-0 bg-transparent px-1 py-0 text-ui text-ink-mute shadow-none"
          >
            <option value="">{listing && addable.length === 0 ? 'no more profiles' : 'add a profile…'}</option>
            {addable.map((p) => (
              <option key={p.id} value={p.id}>
                {p.id} — {p.name}
              </option>
            ))}
          </Select>
        </div>
        {value.map((id) => {
          const p = byId.get(id);
          return (
            <p key={id} className="text-note text-ink-dim">
              <span className="font-mono text-ink">{id}</span>:{' '}
              <span>{p ? profileSummary(p) : 'not a profile this control plane knows'}</span>
            </p>
          );
        })}
        {disabledReason !== null && <p className="text-note text-ink-mute">{disabledReason}</p>}
        {disabledReason === null && note && <p className="text-note text-ink-mute">{note}</p>}
        {loadError && <p className="text-note text-err">Profiles unavailable: {loadError}</p>}
        {listing && listing.errors.length > 0 && (
          <p className="text-note text-warn">
            These profile files don’t load, and every engagement launch fails until they do:{' '}
            {listing.errors.join('; ')}
          </p>
        )}
      </div>
    </details>
  );
}
