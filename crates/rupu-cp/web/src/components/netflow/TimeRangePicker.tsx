// TimeRangePicker — the `?from=`/`?to=` control for the three netflow
// surfaces (global Netflow page, project Network tab, run Network tab).
// Relative presets are the DEFAULT mode, not custom absolute bounds: "what
// did this reach in the last hour" is the question actually asked; a
// specific incident window is the rarer, fallback case. Built entirely on
// existing kit primitives (`ui/Segmented`, `ui/Input`, `ui/Button`) — no
// hand-rolled markup, no new dependency (no date-picker library; see the
// task brief's supply-chain/binary-size constraint).
//
// Fully controlled: `value`/`onChange` are the only state a caller needs.
// The one piece of LOCAL state here is the in-progress custom from/to text
// while the operator is still typing — it is never itself the source of
// truth, and is only committed via `onChange` when "Apply" is clicked, so
// a caller's fetch never re-fires on every keystroke.
//
// Deliberately renders NOTHING about `dropped_total` or any other
// ledger-wide fact — see `NetflowTable.tsx`'s `DroppedBanner` and its own
// comment on why that banner's "whole ledger history" framing must never
// be allowed to read as scoped to whatever this picker currently selects.

import { useState } from 'react';
import { Segmented } from '../ui/Segmented';
import { Input } from '../ui/Input';
import { Button } from '../ui/Button';

/** Every preset this picker can select. `'custom'` is the only one whose
 *  bounds come from the operator rather than being derived from `now`. */
export type TimeRangePreset = 'all' | 'hour' | '24h' | '7d' | 'custom';

/** The picker's controlled value. `from`/`to` are RFC 3339 UTC, already
 *  resolved at selection time (relative presets compute against `now`
 *  once, when clicked — this picker does not itself re-derive them on a
 *  later render, matching the rest of this app's non-polling netflow
 *  surfaces). `'all'` NEVER carries `from`/`to` — see [`toNetflowRange`]. */
export interface TimeRangeValue {
  preset: TimeRangePreset;
  from?: string;
  to?: string;
}

const PRESET_HOURS: Record<'hour' | '24h' | '7d', number> = {
  hour: 1,
  '24h': 24,
  '7d': 24 * 7,
};

const PRESET_OPTIONS: { value: TimeRangePreset; label: string }[] = [
  { value: 'all', label: 'All' },
  { value: 'hour', label: 'Last hour' },
  { value: '24h', label: '24h' },
  { value: '7d', label: '7d' },
  { value: 'custom', label: 'Custom' },
];

/** Relative preset -> its lower bound, resolved against `now`. `'all'` has
 *  no bound (it must never narrow the query); `'custom'`'s bound comes
 *  from the operator, not from this function. */
function presetFrom(preset: TimeRangePreset, now: Date): string | undefined {
  if (preset === 'all' || preset === 'custom') return undefined;
  return new Date(now.getTime() - PRESET_HOURS[preset] * 3_600_000).toISOString();
}

/** A `datetime-local` input's value (local wall-clock, no offset) -> RFC
 *  3339 UTC. `Date`'s parser treats an offset-less date-time string as
 *  LOCAL time (ECMA-262 `Date.parse`), which is exactly the
 *  `datetime-local` contract — this is a plain round-trip through `Date`,
 *  not a hand-rolled timezone calculation. Empty/unparseable input yields
 *  `undefined` (an unset bound), never a bogus timestamp. */
function localInputToRfc3339(value: string): string | undefined {
  if (!value) return undefined;
  const d = new Date(value);
  if (Number.isNaN(d.getTime())) return undefined;
  return d.toISOString();
}

/** The inverse of [`localInputToRfc3339`] — RFC 3339 UTC -> a
 *  `datetime-local` input value in the browser's local timezone. Used only
 *  to PRE-FILL the custom inputs from an already-applied custom value on
 *  mount: without this, a component remount (RunDetail unmounts the
 *  Network tab body on tab switch) would show blank fields while a custom
 *  filter is still genuinely applied — misleading an operator into
 *  thinking their range was silently cleared when it was not. */
function rfc3339ToLocalInputValue(rfc3339: string | undefined): string {
  if (!rfc3339) return '';
  const d = new Date(rfc3339);
  if (Number.isNaN(d.getTime())) return '';
  const pad = (n: number) => String(n).padStart(2, '0');
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}T${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

/** [`TimeRangeValue`] -> the `NetflowRange` the fetch helpers accept.
 *  `undefined` (not `{}`) for both the `'all'` preset and an as-yet-unset
 *  custom range, so a caller can tell "no filter" apart from "an empty
 *  filter object" and skip passing a range argument entirely — see
 *  `lib/netflow.ts`'s `appendRange` and every page call site, which only
 *  pass a second argument when this returns non-`undefined`. */
export function toNetflowRange(
  value: TimeRangeValue,
): { from?: string; to?: string } | undefined {
  if (value.preset === 'all') return undefined;
  if (value.from === undefined && value.to === undefined) return undefined;
  return { from: value.from, to: value.to };
}

export interface TimeRangePickerProps {
  value: TimeRangeValue;
  onChange: (value: TimeRangeValue) => void;
  /** Injectable for deterministic tests; defaults to the real clock. */
  now?: () => Date;
}

export function TimeRangePicker({ value, onChange, now = () => new Date() }: TimeRangePickerProps) {
  const [customOpen, setCustomOpen] = useState(value.preset === 'custom');
  // Lazy initializers so an already-applied custom value survives a
  // remount — see `rfc3339ToLocalInputValue`'s doc comment.
  const [customFrom, setCustomFrom] = useState(() =>
    rfc3339ToLocalInputValue(value.preset === 'custom' ? value.from : undefined),
  );
  const [customTo, setCustomTo] = useState(() =>
    rfc3339ToLocalInputValue(value.preset === 'custom' ? value.to : undefined),
  );

  function selectPreset(preset: TimeRangePreset) {
    if (preset === 'custom') {
      setCustomOpen(true);
      return;
    }
    setCustomOpen(false);
    const from = presetFrom(preset, now());
    onChange(from !== undefined ? { preset, from } : { preset });
  }

  function applyCustom() {
    onChange({
      preset: 'custom',
      from: localInputToRfc3339(customFrom),
      to: localInputToRfc3339(customTo),
    });
  }

  return (
    <div className="flex flex-wrap items-center gap-2">
      <Segmented
        ariaLabel="Time range"
        size="sm"
        options={PRESET_OPTIONS}
        value={customOpen ? 'custom' : value.preset}
        onChange={(v) => selectPreset(v as TimeRangePreset)}
      />
      {customOpen && (
        <div className="flex flex-wrap items-center gap-2">
          <label className="flex items-center gap-1 text-note text-ink-dim">
            From
            <Input
              type="datetime-local"
              aria-label="Custom range start"
              value={customFrom}
              onChange={(e) => setCustomFrom(e.target.value)}
              className="w-auto max-w-none py-1"
            />
          </label>
          <label className="flex items-center gap-1 text-note text-ink-dim">
            To
            <Input
              type="datetime-local"
              aria-label="Custom range end"
              value={customTo}
              onChange={(e) => setCustomTo(e.target.value)}
              className="w-auto max-w-none py-1"
            />
          </label>
          <Button variant="secondary" size="sm" onClick={applyCustom}>
            Apply
          </Button>
        </div>
      )}
    </div>
  );
}

export default TimeRangePicker;
