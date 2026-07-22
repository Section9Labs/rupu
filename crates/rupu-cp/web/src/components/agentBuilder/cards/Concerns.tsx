// Concerns card — `concerns`, a list of `ConcernEntry` (the
// `InlineConcern | IncludeConcern` union from agentSpec.ts, imported
// verbatim — this card never invents its own shape for it). Two entry
// kinds: `include` (a named template, optional mode, optional per-concern
// overrides) and `inline` (a fully-specified concern). `SEVERITIES` is a
// local vocab list — agentSpec.ts exports the `Severity` type but no runtime
// const for it (validate.ts owns its own copy for the same reason).
import {
  type ConcernEntry,
  type ConcernOverride,
  type IncludeConcern,
  type InlineConcern,
  type Severity,
} from '../../../lib/agentBuilder/agentSpec';
import { ChipsInput, LabeledRow, Segmented } from '../controls';
import { cn } from '../../../lib/cn';
import type { CardProps } from './types';

const SEVERITIES: Severity[] = ['info', 'low', 'medium', 'high', 'critical'];
const SEVERITY_OPTIONS = [
  { label: '(none)', value: '' },
  ...SEVERITIES.map((s) => ({ label: s, value: s as string })),
];
const MODE_OPTIONS = [
  { label: 'full', value: 'full' },
  { label: 'index', value: 'index' },
  { label: 'auto', value: 'auto' },
];

function newInclude(): IncludeConcern {
  return { kind: 'include', template: '' };
}
function newInline(): InlineConcern {
  return { kind: 'inline', id: '', name: '', description: '' };
}

export default function Concerns({ draft, patch }: CardProps) {
  const entries = draft.concerns ?? [];

  function setEntries(next: ConcernEntry[]) {
    patch({ concerns: next });
  }
  function removeEntry(i: number) {
    setEntries(entries.filter((_, idx) => idx !== i));
  }
  function updateInclude(i: number, p: Partial<IncludeConcern>) {
    setEntries(entries.map((e, idx) => (idx === i && e.kind === 'include' ? { ...e, ...p } : e)));
  }
  function updateInline(i: number, p: Partial<InlineConcern>) {
    setEntries(entries.map((e, idx) => (idx === i && e.kind === 'inline' ? { ...e, ...p } : e)));
  }
  function addOverride(i: number) {
    setEntries(
      entries.map((e, idx) =>
        idx === i && e.kind === 'include' ? { ...e, overrides: [...(e.overrides ?? []), { id: '' }] } : e,
      ),
    );
  }
  function updateOverride(i: number, j: number, p: Partial<ConcernOverride>) {
    setEntries(
      entries.map((e, idx) => {
        if (idx !== i || e.kind !== 'include') return e;
        return { ...e, overrides: (e.overrides ?? []).map((o, oi) => (oi === j ? { ...o, ...p } : o)) };
      }),
    );
  }
  function removeOverride(i: number, j: number) {
    setEntries(
      entries.map((e, idx) => {
        if (idx !== i || e.kind !== 'include') return e;
        return { ...e, overrides: (e.overrides ?? []).filter((_, oi) => oi !== j) };
      }),
    );
  }

  return (
    <div className="ab-sub">
      {entries.map((entry, i) => (
        <div className="ab-concern" key={i}>
          <div className="ab-top">
            <span className={cn('ab-badge-inc', entry.kind === 'inline' && 'ab-badge-inline')}>{entry.kind}</span>
            {entry.kind === 'include' ? (
              <input
                className="ab-txt mono"
                style={{ flex: 1 }}
                placeholder="template name"
                value={entry.template}
                onChange={(e) => updateInclude(i, { template: e.target.value })}
                aria-label={`concern ${i} template`}
              />
            ) : (
              <input
                className="ab-txt mono"
                style={{ flex: 1 }}
                placeholder="concern id"
                value={entry.id}
                onChange={(e) => updateInline(i, { id: e.target.value })}
                aria-label={`concern ${i} id`}
              />
            )}
            <button
              type="button"
              className="ab-iconbtn"
              aria-label={`remove concern ${i}`}
              onClick={() => removeEntry(i)}
            >
              ✕
            </button>
          </div>

          {entry.kind === 'include' ? (
            <>
              <LabeledRow label="Mode" yamlKey="mode">
                <Segmented<string>
                  options={MODE_OPTIONS}
                  value={entry.mode ?? 'auto'}
                  onChange={(v) => updateInclude(i, { mode: v === 'auto' ? undefined : (v as IncludeConcern['mode']) })}
                />
              </LabeledRow>
              <div className="ab-sub">
                <label className="ab-lab">Overrides</label>
                {(entry.overrides ?? []).map((o, j) => (
                  <div key={j} className="ab-row" style={{ gap: 6 }}>
                    <div className="ab-subrow">
                      <input
                        className="ab-txt mono"
                        placeholder="concern id"
                        value={o.id}
                        onChange={(e) => updateOverride(i, j, { id: e.target.value })}
                        aria-label={`concern ${i} override ${j} id`}
                      />
                      <Segmented<string>
                        options={SEVERITY_OPTIONS}
                        value={o.severity ?? ''}
                        onChange={(v) => updateOverride(i, j, { severity: (v || undefined) as Severity | undefined })}
                      />
                      <button
                        type="button"
                        className="ab-iconbtn"
                        aria-label={`remove concern ${i} override ${j}`}
                        onClick={() => removeOverride(i, j)}
                      >
                        ✕
                      </button>
                    </div>
                    <ChipsInput
                      list={o.applicableGlobs ?? []}
                      placeholder="applicable glob…"
                      onChange={(next) => updateOverride(i, j, { applicableGlobs: next })}
                    />
                  </div>
                ))}
                <button type="button" className="ab-tinybtn" onClick={() => addOverride(i)}>
                  + override
                </button>
              </div>
            </>
          ) : (
            <>
              <LabeledRow label="Name" yamlKey="name">
                <input
                  className="ab-txt"
                  value={entry.name}
                  onChange={(e) => updateInline(i, { name: e.target.value })}
                  aria-label={`concern ${i} name`}
                />
              </LabeledRow>
              <LabeledRow label="Description" yamlKey="description">
                <textarea
                  className="ab-txt"
                  value={entry.description}
                  onChange={(e) => updateInline(i, { description: e.target.value })}
                  aria-label={`concern ${i} description`}
                />
              </LabeledRow>
              <LabeledRow label="Severity" yamlKey="severity">
                <Segmented<string>
                  options={SEVERITY_OPTIONS}
                  value={entry.severity ?? ''}
                  onChange={(v) => updateInline(i, { severity: (v || undefined) as Severity | undefined })}
                />
              </LabeledRow>
              <LabeledRow label="Applicable globs" yamlKey="applicable_globs">
                <ChipsInput
                  list={entry.applicableGlobs ?? []}
                  placeholder="src/**/*.ts"
                  onChange={(next) => updateInline(i, { applicableGlobs: next })}
                />
              </LabeledRow>
            </>
          )}
        </div>
      ))}
      <div style={{ display: 'flex', gap: 7 }}>
        <button
          type="button"
          className="ab-tinybtn"
          onClick={() => setEntries([...entries, newInclude()])}
        >
          + include template
        </button>
        <button
          type="button"
          className="ab-tinybtn"
          onClick={() => setEntries([...entries, newInline()])}
        >
          + inline concern
        </button>
      </div>
    </div>
  );
}
