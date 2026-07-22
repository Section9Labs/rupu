// AutoflowCard — Autoflow authoring card for the Settings inspector (Task 6),
// rendered only when `workflowEditorUi === 'next'` (WorkflowSettingsForm
// decides that; this component doesn't gate itself).
//
// An enable toggle at top; when off, only the toggle renders. When on, six
// sub-sections (entity / selector / author gate / reconcile / workspace /
// outcome) bind form controls to an `AutoflowModel`.
//
// The FULL model is held in local state (mirroring InputsCard's `rows`
// pattern), seeded from `readAutoflow(rest)` and reconciled ONLY when
// `rest.autoflow` changes to something this component didn't itself just
// write (tracked via `lastWrittenAutoflowRef`). This is the same
// stable-identity fix as Task 5's InputsCard: deriving the model fresh from
// `rest` on every render would clobber in-progress edits (a chip about to be
// added, a still-blank text field) on our own echoed-back `onRest`
// round-trip. All add/remove chip lists (labels_all/any/none, authors,
// wake_on) live inside this single local model rather than as separate
// per-list local state, so there's one source of truth and no risk of the
// list being silently re-derived out from under an in-flight edit.
//
// Every mutation reads the CURRENT local model, patches it, and writes back
// via `writeAutoflow` — never hand-building the `autoflow:` shape here.

import { useEffect, useRef, useState } from 'react';
import {
  readAutoflow,
  writeAutoflow,
  contractOutputKeys,
  type AutoflowModel,
  type AutoflowSelectorModel,
  type AutoflowEntity,
  type AutoflowIssueState,
  type DraftFilter,
  type AuthorScope,
  type SkipAction,
  type AutoflowClaimKey,
  type AutoflowWorkspaceStrategy,
} from '../../../lib/workflowMeta';
import { Button } from '../../ui/Button';

interface AutoflowCardProps {
  rest: Record<string, unknown>;
  onRest: (rest: Record<string, unknown>) => void;
}

const fieldCls =
  'w-full rounded-md border border-border bg-panel px-2.5 py-1.5 text-lead text-ink placeholder:text-ink-mute focus:border-brand-500 focus:outline-none';
const labelCls = 'mb-1 block text-ui font-semibold uppercase tracking-wide text-ink-dim';

const EMPTY_MODEL: AutoflowModel = { enabled: false, entity: 'issue', selector: {}, wake_on: [] };

const ENTITY_OPTIONS: { value: AutoflowEntity; label: string }[] = [
  { value: 'issue', label: 'issue' },
  { value: 'pull_request', label: 'pull_request' },
];
const STATE_OPTIONS: AutoflowIssueState[] = ['open', 'closed'];
const DRAFT_OPTIONS: DraftFilter[] = ['include', 'exclude', 'only'];
const AUTHORS_FROM_OPTIONS: { value: AuthorScope | undefined; label: string }[] = [
  { value: 'collaborators', label: 'collaborators' },
  { value: 'org_members', label: 'org_members' },
  { value: undefined, label: 'anyone' },
];
const ON_SKIP_OPTIONS: SkipAction[] = ['skip', 'label_needs_human'];
const CLAIM_KEY_OPTIONS: AutoflowClaimKey[] = ['issue', 'pr_head_sha'];
const WORKSPACE_STRATEGY_OPTIONS: AutoflowWorkspaceStrategy[] = ['worktree', 'in_place'];

type SelectorListField = 'labels_all' | 'labels_any' | 'labels_none' | 'authors';

export default function AutoflowCard({ rest, onRest }: AutoflowCardProps) {
  const [model, setModel] = useState<AutoflowModel>(() => readAutoflow(rest) ?? EMPTY_MODEL);
  const lastWrittenAutoflowRef = useRef<unknown>(rest.autoflow);

  useEffect(() => {
    if (rest.autoflow !== lastWrittenAutoflowRef.current) {
      lastWrittenAutoflowRef.current = rest.autoflow;
      setModel(readAutoflow(rest) ?? EMPTY_MODEL);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [rest.autoflow]);

  // Pending "add chip" text, keyed by selector-list field name or 'wake_on'.
  const [pending, setPending] = useState<Record<string, string>>({});

  function commit(next: AutoflowModel): void {
    setModel(next);
    const nextRest = writeAutoflow(rest, next);
    lastWrittenAutoflowRef.current = nextRest.autoflow;
    onRest(nextRest);
  }

  function patchSelector(p: Partial<AutoflowSelectorModel>): void {
    commit({ ...model, selector: { ...model.selector, ...p } });
  }

  function toggleEnabled(): void {
    commit({ ...model, enabled: !model.enabled });
  }

  function setEntity(entity: AutoflowEntity): void {
    // Draft/base are only schema-valid for pull_request — clear both on
    // switch so an issue autoflow never carries stale values.
    const selector = { ...model.selector };
    if (entity !== 'pull_request') {
      delete selector.draft;
      delete selector.base;
    }
    commit({ ...model, entity, selector });
  }

  function toggleState(s: AutoflowIssueState): void {
    const set = new Set(model.selector.states ?? []);
    if (set.has(s)) set.delete(s);
    else set.add(s);
    const states = Array.from(set);
    patchSelector({ states: states.length > 0 ? states : undefined });
  }

  function selectorList(field: SelectorListField): string[] {
    return model.selector[field] ?? [];
  }
  function addSelectorListValue(field: SelectorListField): void {
    const v = (pending[field] ?? '').trim();
    if (v === '') return;
    const cur = selectorList(field);
    if (!cur.includes(v)) patchSelector({ [field]: [...cur, v] });
    setPending((prev) => ({ ...prev, [field]: '' }));
  }
  function removeSelectorListValue(field: SelectorListField, v: string): void {
    patchSelector({ [field]: selectorList(field).filter((x) => x !== v) });
  }

  function addWakeOn(): void {
    const v = (pending.wake_on ?? '').trim();
    if (v === '' || model.wake_on.includes(v)) return;
    commit({ ...model, wake_on: [...model.wake_on, v] });
    setPending((prev) => ({ ...prev, wake_on: '' }));
  }
  function removeWakeOn(v: string): void {
    commit({ ...model, wake_on: model.wake_on.filter((x) => x !== v) });
  }

  const defaultClaimKey: AutoflowClaimKey = model.entity === 'pull_request' ? 'pr_head_sha' : 'issue';
  function setClaimKey(key: AutoflowClaimKey): void {
    commit({ ...model, claim: { ...(model.claim ?? { key: defaultClaimKey }), key } });
  }
  function setClaimTtl(ttl: string): void {
    const trimmed = ttl.trim();
    commit({ ...model, claim: { key: model.claim?.key ?? defaultClaimKey, ttl: trimmed === '' ? undefined : trimmed } });
  }

  function setWorkspaceStrategy(strategy: AutoflowWorkspaceStrategy): void {
    commit({ ...model, workspace: { ...(model.workspace ?? { strategy: 'worktree' }), strategy } });
  }
  function setWorkspaceBranch(branch: string): void {
    const trimmed = branch.trim();
    commit({
      ...model,
      workspace: { strategy: model.workspace?.strategy ?? 'worktree', branch: trimmed === '' ? undefined : trimmed },
    });
  }

  const outputKeys = contractOutputKeys(rest);
  function setOutcome(output: string): void {
    commit({ ...model, outcome: output === '' ? undefined : { output } });
  }

  function renderChipField(
    label: string,
    field: SelectorListField | 'wake_on',
    values: string[],
    onAdd: () => void,
    onRemove: (v: string) => void,
  ) {
    return (
      <div key={field}>
        <span className={labelCls}>{label}</span>
        {values.length > 0 && (
          <div className="wfx-chiprow mb-1.5">
            {values.map((v) => (
              <span key={v} className="wfx-chip">
                {v}
                <button type="button" onClick={() => onRemove(v)} aria-label={`Remove ${label} value ${v}`}>
                  ×
                </button>
              </span>
            ))}
          </div>
        )}
        <div className="flex items-center gap-2">
          <input
            type="text"
            value={pending[field] ?? ''}
            onChange={(e) => setPending((prev) => ({ ...prev, [field]: e.target.value }))}
            aria-label={`Autoflow ${label} value`}
            placeholder="add value"
            className={fieldCls}
            onKeyDown={(e) => {
              if (e.key === 'Enter') {
                e.preventDefault();
                onAdd();
              }
            }}
          />
          <Button variant="secondary" size="sm" aria-label={`Add ${label} value`} className="shrink-0" onClick={onAdd}>
            Add
          </Button>
        </div>
      </div>
    );
  }

  return (
    <div className="wfx-card" data-testid="autoflow-card">
      <div className="wfx-card-h">
        <span>Autoflow</span>
        <button
          type="button"
          role="switch"
          aria-checked={model.enabled}
          aria-label="Autoflow enabled"
          onClick={toggleEnabled}
          className={`wfx-toggle${model.enabled ? ' wfx-on' : ''}`}
        >
          <span className="wfx-toggle-sw" />
        </button>
      </div>

      {model.enabled && (
        <div className="wfx-card-b">
          <div>
            <span className={labelCls}>Entity</span>
            <div className="wfx-seg" role="group" aria-label="Autoflow entity">
              {ENTITY_OPTIONS.map((opt) => (
                <button
                  key={opt.value}
                  type="button"
                  aria-pressed={model.entity === opt.value}
                  onClick={() => setEntity(opt.value)}
                >
                  {opt.label}
                </button>
              ))}
            </div>
          </div>

          <div className="wfx-subcard">
            <div className="wfx-subcard-h">Selector</div>

            <div>
              <span className={labelCls}>States</span>
              <div className="wfx-seg" role="group" aria-label="Autoflow states">
                {STATE_OPTIONS.map((s) => (
                  <button
                    key={s}
                    type="button"
                    aria-pressed={(model.selector.states ?? []).includes(s)}
                    onClick={() => toggleState(s)}
                  >
                    {s}
                  </button>
                ))}
              </div>
            </div>

            {renderChipField(
              'labels_all',
              'labels_all',
              selectorList('labels_all'),
              () => addSelectorListValue('labels_all'),
              (v) => removeSelectorListValue('labels_all', v),
            )}
            {renderChipField(
              'labels_any',
              'labels_any',
              selectorList('labels_any'),
              () => addSelectorListValue('labels_any'),
              (v) => removeSelectorListValue('labels_any', v),
            )}
            {renderChipField(
              'labels_none',
              'labels_none',
              selectorList('labels_none'),
              () => addSelectorListValue('labels_none'),
              (v) => removeSelectorListValue('labels_none', v),
            )}

            <label className="block">
              <span className={labelCls}>Limit</span>
              <input
                type="number"
                value={model.selector.limit ?? ''}
                onChange={(e) =>
                  patchSelector({ limit: e.target.value === '' ? undefined : Number(e.target.value) })
                }
                aria-label="Autoflow limit"
                className={fieldCls}
              />
            </label>

            {model.entity === 'pull_request' && (
              <>
                <div>
                  <span className={labelCls}>Draft</span>
                  <div className="wfx-seg" role="group" aria-label="Autoflow draft filter">
                    {DRAFT_OPTIONS.map((d) => (
                      <button
                        key={d}
                        type="button"
                        aria-pressed={model.selector.draft === d}
                        onClick={() => patchSelector({ draft: d })}
                      >
                        {d}
                      </button>
                    ))}
                  </div>
                </div>
                <label className="block">
                  <span className={labelCls}>Base branch</span>
                  <input
                    type="text"
                    value={model.selector.base ?? ''}
                    onChange={(e) => patchSelector({ base: e.target.value === '' ? undefined : e.target.value })}
                    aria-label="Autoflow base branch"
                    className={`${fieldCls} font-mono`}
                  />
                </label>
              </>
            )}

            {renderChipField(
              'authors',
              'authors',
              selectorList('authors'),
              () => addSelectorListValue('authors'),
              (v) => removeSelectorListValue('authors', v),
            )}
          </div>

          <div className="wfx-subcard">
            <div className="wfx-subcard-h">Author gate</div>
            <div>
              <span className={labelCls}>Authors from</span>
              <div className="wfx-seg" role="group" aria-label="Autoflow authors from">
                {AUTHORS_FROM_OPTIONS.map((opt) => (
                  <button
                    key={opt.label}
                    type="button"
                    aria-pressed={model.selector.authors_from === opt.value}
                    onClick={() => patchSelector({ authors_from: opt.value })}
                  >
                    {opt.label}
                  </button>
                ))}
              </div>
            </div>
            <div>
              <span className={labelCls}>On skip</span>
              <div className="wfx-seg" role="group" aria-label="Autoflow on skip">
                {ON_SKIP_OPTIONS.map((s) => (
                  <button
                    key={s}
                    type="button"
                    aria-pressed={model.selector.on_skip === s}
                    onClick={() => patchSelector({ on_skip: s })}
                  >
                    {s}
                  </button>
                ))}
              </div>
            </div>
          </div>

          <div className="wfx-subcard">
            <div className="wfx-subcard-h">Reconcile</div>
            <div className="wfx-row-two">
              <label className="block">
                <span className={labelCls}>Every</span>
                <input
                  type="text"
                  value={model.reconcile_every ?? ''}
                  onChange={(e) =>
                    commit({ ...model, reconcile_every: e.target.value === '' ? undefined : e.target.value })
                  }
                  aria-label="Autoflow reconcile every"
                  placeholder="10m"
                  className={`${fieldCls} font-mono`}
                />
              </label>
              <label className="block">
                <span className={labelCls}>Claim TTL</span>
                <input
                  type="text"
                  value={model.claim?.ttl ?? ''}
                  onChange={(e) => setClaimTtl(e.target.value)}
                  aria-label="Autoflow claim ttl"
                  placeholder="3h"
                  className={`${fieldCls} font-mono`}
                />
              </label>
            </div>
            <div>
              <span className={labelCls}>Claim key</span>
              <div className="wfx-seg" role="group" aria-label="Autoflow claim key">
                {CLAIM_KEY_OPTIONS.map((k) => (
                  <button
                    key={k}
                    type="button"
                    aria-pressed={(model.claim?.key ?? defaultClaimKey) === k}
                    onClick={() => setClaimKey(k)}
                  >
                    {k}
                  </button>
                ))}
              </div>
            </div>
            {renderChipField('wake_on', 'wake_on', model.wake_on, addWakeOn, removeWakeOn)}
          </div>

          <div className="wfx-subcard">
            <div className="wfx-subcard-h">Workspace</div>
            <div>
              <span className={labelCls}>Strategy</span>
              <div className="wfx-seg" role="group" aria-label="Autoflow workspace strategy">
                {WORKSPACE_STRATEGY_OPTIONS.map((s) => (
                  <button
                    key={s}
                    type="button"
                    aria-pressed={(model.workspace?.strategy ?? 'worktree') === s}
                    onClick={() => setWorkspaceStrategy(s)}
                  >
                    {s}
                  </button>
                ))}
              </div>
            </div>
            <label className="block">
              <span className={labelCls}>Branch</span>
              <input
                type="text"
                value={model.workspace?.branch ?? ''}
                onChange={(e) => setWorkspaceBranch(e.target.value)}
                aria-label="Autoflow workspace branch"
                className={`${fieldCls} font-mono`}
              />
            </label>
          </div>

          <div className="wfx-subcard">
            <div className="wfx-subcard-h">Outcome</div>
            <label className="block">
              <span className={labelCls}>Output</span>
              <select
                value={model.outcome?.output ?? ''}
                onChange={(e) => setOutcome(e.target.value)}
                aria-label="Autoflow outcome output"
                disabled={outputKeys.length === 0}
                className={fieldCls}
              >
                <option value="">— none —</option>
                {outputKeys.map((k) => (
                  <option key={k} value={k}>
                    {k}
                  </option>
                ))}
              </select>
              {outputKeys.length === 0 && (
                <span className="mt-1 block text-note text-ink-mute">
                  define contracts.outputs in the YAML tab
                </span>
              )}
              {outputKeys.length > 0 && (
                <span className="mt-1 block text-note text-ink-mute">Reconcile stops when this output is emitted.</span>
              )}
            </label>
          </div>
        </div>
      )}
    </div>
  );
}
