// Settings — global CP configuration. Fetches the resolved `GET /api/config`
// view (effective values + per-key provenance) and renders it as a tabbed
// form: General / Providers / Autoflow / SCM-Issues / Pricing / CP-Runtime.
// Each typed field shows its resolved value, a provenance badge (which layer
// won: global/project/env/default), and a lock toggle that adds/removes the
// key from the GLOBAL `[policy].lock` enforced-key list (`PUT
// /api/config/policy`, applied immediately — independent of the Save button).
// Edited fields accumulate in a `patch` and are submitted together via `Save
// changes` (`PUT /api/config/global`). No secret/token VALUE is ever fetched
// or rendered here — the CP's own bearer token only ever appears as
// `status.token_set: bool` (Bearer token row on the CP-Runtime tab), and it is
// never editable from this form.
//
// Three more tabs round out the page:
//  - Raw: the global `config.toml` shown highlighted (read-only reference)
//    plus an editable textarea seeded from `raw_global`. Save posts the full
//    text as `{ raw }` to `PUT /api/config/global`, which validates against
//    the typed schema before writing anything — there is no separate
//    non-persisting "validate" endpoint on the backend, so Save both
//    validates and persists in one step; a 400 renders the server's message
//    inline next to the editor.
//  - Policy: a consolidated view of every dotted key with known provenance,
//    each with a lock checkbox seeded from `provenance[key].locked`. Edits
//    are staged locally and committed together via a Save button that PUTs
//    the whole updated lock list — independent of (and functionally
//    overlapping with) the per-field lock toggle above.
//  - Runtime status: read-only — bind address, the bearer token masked as
//    `••• set` / `not set` (never a value), and which keys need a process
//    restart to take effect.

import { useEffect, useRef, useState, type ReactNode } from 'react';
import {
  Activity,
  Cpu,
  DollarSign,
  FileCode,
  GitBranch,
  Lock as LockIcon,
  Server,
  SlidersHorizontal,
  Workflow,
} from 'lucide-react';
import { api, ApiError, type ConfigRuntimeStatus, type ConfigView } from '../lib/api';
import { cn } from '../lib/cn';
import { TabBar, TabButton } from '../components/TabBar';
import { Button } from '../components/ui/Button';
import {
  getPath,
  sameSet,
  EmptyTabState,
  type TabProps,
  GeneralTab,
  ProvidersTab,
  AutoflowTab,
  ScmTab,
  PricingTab,
  CpFieldTab,
  RawTab,
} from '../components/ConfigEditor';
import { FieldGroup, toggleInputCls } from '../components/settings/ConfigField';

type SettingsTab =
  | 'general'
  | 'providers'
  | 'autoflow'
  | 'scm'
  | 'pricing'
  | 'cp-runtime'
  | 'raw'
  | 'policy'
  | 'runtime-status';

// ---------------------------------------------------------------------------
// Small status-tile primitive shared by the CP-Runtime and Runtime-status
// tabs below — a labelled `dt`/`dd` pair on a subtle card background, instead
// of the bare label-over-value stack the page used to render.
// ---------------------------------------------------------------------------

function InfoTile({ label, value, mono, wide }: { label: string; value: ReactNode; mono?: boolean; wide?: boolean }) {
  return (
    <div className={cn('rounded-lg border border-border bg-surface/40 px-3 py-2.5', wide && 'sm:col-span-2')}>
      <dt className="text-note font-medium uppercase tracking-wide text-ink-mute">{label}</dt>
      <dd className={cn('mt-1 text-sm text-ink', mono && 'font-mono break-all')}>{value}</dd>
    </div>
  );
}

// ---------------------------------------------------------------------------
// CP-Runtime tab — the shared `cp.max_workspace_bytes` field plus a read-only
// runtime-status block (bind address / masked token / restart-required keys)
// that is GLOBAL-only, so it stays local to Settings rather than living in
// the shared ConfigEditor module.
// ---------------------------------------------------------------------------

function CpRuntimeTab({
  prov,
  lockList,
  fieldValue,
  onChange,
  onToggleLock,
  status,
}: Omit<TabProps, 'eff'> & { status: ConfigRuntimeStatus }) {
  return (
    <div className="space-y-4">
      <CpFieldTab
        prov={prov}
        lockList={lockList}
        fieldValue={fieldValue}
        onChange={onChange}
        onToggleLock={onToggleLock}
      />

      <FieldGroup title="Runtime status" description="Read-only — reflects the running rupu cp serve process.">
        <dl className="grid grid-cols-1 gap-3 py-3 sm:grid-cols-2">
          <InfoTile label="Bind address" value={status.bind} mono />
          <InfoTile label="Bearer token" value={status.token_set ? '••• set' : 'not configured'} />
          {status.restart_required_keys.length > 0 && (
            <InfoTile label="Requires restart to apply" value={status.restart_required_keys.join(', ')} mono wide />
          )}
        </dl>
      </FieldGroup>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Policy tab
// ---------------------------------------------------------------------------

/** Group dotted keys by their top-level namespace (`autoflow.*`, `scm.*`, …)
 *  so the consolidated lock list reads as sections instead of one long flat
 *  list of unrelated keys. Root-level keys (no dot) fall into "general". */
function groupPolicyKeys(keys: string[]): Array<[string, string[]]> {
  const groups = new Map<string, string[]>();
  for (const key of keys) {
    const ns = key.includes('.') ? key.split('.')[0] : 'general';
    const list = groups.get(ns) ?? [];
    list.push(key);
    groups.set(ns, list);
  }
  return Array.from(groups.entries()).sort(([a], [b]) => a.localeCompare(b));
}

function PolicyTab({
  keys,
  locks,
  onToggle,
  onSave,
  saving,
  error,
  dirty,
}: {
  keys: string[];
  locks: string[];
  onToggle: (key: string) => void;
  onSave: () => void;
  saving: boolean;
  error: string | null;
  dirty: boolean;
}) {
  if (keys.length === 0) {
    return <EmptyTabState text="No resolved keys yet — nothing to lock." />;
  }
  return (
    <div className="space-y-4">
      <p className="text-sm text-ink-dim">
        Keys enforced by the GLOBAL policy lock — a locked key can never be overridden by a project
        layer. Consolidated view of every key from the General / Providers / Autoflow / SCM-Issues /
        Pricing / CP-Runtime tabs; toggling here is equivalent to the lock glyph on those tabs.
      </p>

      {groupPolicyKeys(keys).map(([ns, nsKeys]) => (
        <FieldGroup key={ns} title={ns}>
          {nsKeys.map((key) => (
            <div key={key} className="flex items-center justify-between gap-3 py-2">
              <label htmlFor={`policy-lock-${key}`} className="min-w-0 flex-1 truncate font-mono text-sm text-ink">
                {key}
              </label>
              <input
                id={`policy-lock-${key}`}
                type="checkbox"
                checked={locks.includes(key)}
                onChange={() => onToggle(key)}
                aria-label={key}
                className={toggleInputCls}
              />
            </div>
          ))}
        </FieldGroup>
      ))}

      {error && (
        <div role="alert" className="rounded-lg border border-err/30 bg-err-bg px-4 py-3 text-sm text-err">
          {error}
        </div>
      )}

      <div className="flex justify-end">
        <Button onClick={onSave} disabled={saving || !dirty}>
          {saving ? 'Saving…' : 'Save policy'}
        </Button>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Runtime status tab
// ---------------------------------------------------------------------------

function RuntimeStatusTab({ status }: { status: ConfigRuntimeStatus }) {
  return (
    <div className="space-y-4">
      <dl className="grid grid-cols-1 gap-3 sm:grid-cols-2">
        <InfoTile label="Bind address" value={status.bind} mono />
        <InfoTile label="CP bearer token" value={status.token_set ? '••• set' : 'not set'} />
      </dl>

      {status.restart_required_keys.length > 0 ? (
        <div className="rounded-lg border border-warn/30 bg-warn-bg px-4 py-3 text-sm text-warn">
          Requires restarting <code className="font-mono">rupu cp serve</code> to apply changes to:{' '}
          <span className="font-mono">{status.restart_required_keys.join(', ')}</span>.
        </div>
      ) : (
        <p className="text-sm text-ink-dim">No pending changes require a restart.</p>
      )}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Page
// ---------------------------------------------------------------------------

export default function Settings() {
  const [configView, setConfigView] = useState<ConfigView | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [pendingPatch, setPendingPatch] = useState<Record<string, unknown>>({});
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);
  const [saveInfo, setSaveInfo] = useState<string | null>(null);
  const [readOnly, setReadOnly] = useState(false);
  const [lockList, setLockList] = useState<string[]>([]);
  const [lockError, setLockError] = useState<string | null>(null);
  const [tab, setTab] = useState<SettingsTab>('general');

  // ── Raw TOML tab state ──────────────────────────────────────────────────
  // `rawBaselineRef` tracks the last raw text known from the server. On
  // reload, the draft is only replaced with the fresh `raw_global` when it
  // still matches that baseline (i.e. the user has no in-flight, unsaved
  // edit) — otherwise an unrelated Save (e.g. a form-tab patch, which also
  // rewrites the file) would silently clobber the Raw tab's draft.
  const [rawDraft, setRawDraft] = useState('');
  const [rawSaving, setRawSaving] = useState(false);
  const [rawError, setRawError] = useState<string | null>(null);
  const rawBaselineRef = useRef('');

  // ── Policy tab state ────────────────────────────────────────────────────
  // `null` means "no staged edits yet" — the checkboxes mirror `lockList`
  // directly until the operator toggles one, at which point edits accumulate
  // here and are committed together via the Policy tab's own Save button.
  const [policyDraft, setPolicyDraft] = useState<string[] | null>(null);
  const [policySaving, setPolicySaving] = useState(false);
  const [policySaveError, setPolicySaveError] = useState<string | null>(null);
  const policyBaselineRef = useRef<string[]>([]);

  function reload(): Promise<void> {
    return api
      .getConfig()
      .then((data) => {
        setConfigView(data);
        const locks = getPath(data.effective, 'policy.lock');
        const nextLocks = Array.isArray(locks) ? locks.filter((l): l is string => typeof l === 'string') : [];
        setLockList(nextLocks);
        // Capture the OLD baseline before overwriting the ref below — the
        // functional updaters run later (React's render phase), by which
        // point the ref would already hold the NEW value if reassigned first,
        // making the "still in sync" comparison always false.
        const prevRawBaseline = rawBaselineRef.current;
        setRawDraft((prevDraft) => (prevDraft === prevRawBaseline ? data.raw_global : prevDraft));
        rawBaselineRef.current = data.raw_global;
        // Only clear staged Policy-tab edits when they're still in sync with
        // the previously-known lock list — an unrelated reload (e.g. a
        // General-tab Save, which rewrites the same file) must not discard an
        // in-progress, unsaved Policy edit.
        const prevPolicyBaseline = policyBaselineRef.current;
        setPolicyDraft((prevDraft) => (prevDraft === null || sameSet(prevDraft, prevPolicyBaseline) ? null : prevDraft));
        policyBaselineRef.current = nextLocks;
        setLoadError(null);
      })
      .catch((e: unknown) => {
        setLoadError(e instanceof Error ? e.message : 'Failed to load config');
      });
  }

  useEffect(() => {
    void reload();
  }, []);

  function fieldValue(key: string): unknown {
    if (Object.prototype.hasOwnProperty.call(pendingPatch, key)) return pendingPatch[key];
    return configView ? getPath(configView.effective, key) : undefined;
  }

  function handleFieldChange(key: string, value: unknown) {
    // Clearing a field (select "—" / emptying a text or number input) yields
    // `undefined`. The write path (`PUT /api/config/global`) has no way to
    // UNSET a key in v1 — sending `null` 400s server-side, and
    // `JSON.stringify` silently drops `undefined`-valued keys, which would
    // otherwise produce an empty (or partial) patch that "succeeds" as a
    // silent no-op. So an `undefined` transition is never staged as an edit;
    // drop the key from the pending patch instead, which reverts the field's
    // displayed value back to its current resolved value (see `fieldValue`).
    // Unsetting a key is only possible via the Raw TOML tab.
    setSaveInfo(null);
    setPendingPatch((prev) => {
      if (value === undefined) {
        if (!(key in prev)) return prev;
        const next = { ...prev };
        delete next[key];
        return next;
      }
      return { ...prev, [key]: value };
    });
  }

  async function handleToggleLock(key: string) {
    const prev = lockList;
    const next = prev.includes(key) ? prev.filter((k) => k !== key) : [...prev, key];
    setLockList(next);
    setLockError(null);
    try {
      await api.putPolicy(next);
      await reload();
    } catch (e: unknown) {
      setLockList(prev);
      setLockError(e instanceof Error ? e.message : 'Failed to update lock policy');
    }
  }

  async function handleSave() {
    if (Object.keys(pendingPatch).length === 0) return;
    // Defensive: `handleFieldChange` already keeps `undefined`-valued entries
    // out of `pendingPatch`, but never send one to the wire even if that
    // invariant is somehow violated — `JSON.stringify` drops `undefined`
    // keys, which would otherwise turn into a silently-successful no-op patch.
    const effectivePatch = Object.fromEntries(
      Object.entries(pendingPatch).filter(([, v]) => v !== undefined),
    );
    if (Object.keys(effectivePatch).length === 0) {
      setSaveError(null);
      setSaveInfo('No changes to save.');
      setPendingPatch({});
      return;
    }
    setSaving(true);
    setSaveError(null);
    setSaveInfo(null);
    setReadOnly(false);
    try {
      await api.putGlobalConfig({ patch: effectivePatch });
      setPendingPatch({});
      await reload();
    } catch (e: unknown) {
      if (e instanceof ApiError && e.status === 501) {
        setReadOnly(true);
      } else {
        setSaveError(e instanceof Error ? e.message : 'Failed to save config');
      }
    } finally {
      setSaving(false);
    }
  }

  async function handleRawSave() {
    setRawSaving(true);
    setRawError(null);
    setReadOnly(false);
    try {
      await api.putGlobalConfig({ raw: rawDraft });
      await reload();
    } catch (e: unknown) {
      if (e instanceof ApiError && e.status === 501) {
        setReadOnly(true);
      } else {
        setRawError(e instanceof Error ? e.message : 'Failed to save raw config');
      }
    } finally {
      setRawSaving(false);
    }
  }

  function handlePolicyToggle(key: string) {
    const current = policyDraft ?? lockList;
    const next = current.includes(key) ? current.filter((k) => k !== key) : [...current, key];
    setPolicyDraft(next);
  }

  async function handlePolicySave() {
    const next = policyDraft ?? lockList;
    setPolicySaving(true);
    setPolicySaveError(null);
    try {
      await api.putPolicy(next);
      await reload();
    } catch (e: unknown) {
      setPolicySaveError(e instanceof Error ? e.message : 'Failed to update lock policy');
    } finally {
      setPolicySaving(false);
    }
  }

  if (loadError) {
    return (
      <div className="p-8">
        <div className="rounded-lg border border-err/30 bg-err-bg px-4 py-3 text-sm text-err">{loadError}</div>
      </div>
    );
  }

  if (configView === null) {
    return (
      <div className="p-8">
        <div className="text-sm text-ink-dim">Loading settings…</div>
      </div>
    );
  }

  const eff = configView.effective;
  const prov = configView.provenance;
  const dirtyCount = Object.keys(pendingPatch).length;

  return (
    <div className="p-8 space-y-6">
      <header className="flex items-center justify-between">
        <div>
          <h1 className="text-2xl font-semibold text-ink">Settings</h1>
          <p className="mt-1 text-sm text-ink-dim">
            Global rupu configuration, resolved from{' '}
            <span className="font-mono">~/.rupu/config.toml</span>.
          </p>
        </div>
        <div className="flex items-center gap-3">
          {dirtyCount > 0 && (
            <span className="text-note text-ink-dim">
              {dirtyCount} unsaved change{dirtyCount === 1 ? '' : 's'}
            </span>
          )}
          <Button onClick={() => void handleSave()} disabled={dirtyCount === 0 || saving}>
            {saving ? 'Saving…' : 'Save changes'}
          </Button>
        </div>
      </header>

      {readOnly && (
        <div className="rounded-lg border border-warn/30 bg-warn-bg px-4 py-3 text-sm text-warn">
          This is a read-only deploy — editing config requires <code className="font-mono">rupu cp serve</code>.
        </div>
      )}
      {saveError && (
        <div role="alert" className="rounded-lg border border-err/30 bg-err-bg px-4 py-3 text-sm text-err">
          {saveError}
        </div>
      )}
      {saveInfo && (
        <div className="rounded-lg border border-border bg-surface px-4 py-3 text-sm text-ink-dim">
          {saveInfo}
        </div>
      )}
      {lockError && (
        <div role="alert" className="rounded-lg border border-err/30 bg-err-bg px-4 py-3 text-sm text-err">
          {lockError}
        </div>
      )}

      <div className="-mx-8">
        <TabBar>
          <TabButton active={tab === 'general'} onClick={() => setTab('general')} icon={SlidersHorizontal} label="General" />
          <TabButton active={tab === 'providers'} onClick={() => setTab('providers')} icon={Cpu} label="Providers" />
          <TabButton active={tab === 'autoflow'} onClick={() => setTab('autoflow')} icon={Workflow} label="Autoflow" />
          <TabButton active={tab === 'scm'} onClick={() => setTab('scm')} icon={GitBranch} label="SCM / Issues" />
          <TabButton active={tab === 'pricing'} onClick={() => setTab('pricing')} icon={DollarSign} label="Pricing" />
          <TabButton active={tab === 'cp-runtime'} onClick={() => setTab('cp-runtime')} icon={Server} label="CP-Runtime" />
          <div aria-hidden="true" className="mx-1 h-5 w-px shrink-0 bg-border" />
          <TabButton active={tab === 'raw'} onClick={() => setTab('raw')} icon={FileCode} label="Raw" />
          <TabButton active={tab === 'policy'} onClick={() => setTab('policy')} icon={LockIcon} label="Policy" />
          <TabButton
            active={tab === 'runtime-status'}
            onClick={() => setTab('runtime-status')}
            icon={Activity}
            label="Runtime status"
          />
        </TabBar>
      </div>

      <section className="bg-panel border border-border rounded-xl shadow-card px-5 py-4">
        {tab === 'general' && (
          <GeneralTab
            eff={eff}
            prov={prov}
            lockList={lockList}
            fieldValue={fieldValue}
            onChange={handleFieldChange}
            onToggleLock={handleToggleLock}
          />
        )}
        {tab === 'providers' && (
          <ProvidersTab
            eff={eff}
            prov={prov}
            lockList={lockList}
            fieldValue={fieldValue}
            onChange={handleFieldChange}
            onToggleLock={handleToggleLock}
          />
        )}
        {tab === 'autoflow' && (
          <AutoflowTab
            eff={eff}
            prov={prov}
            lockList={lockList}
            fieldValue={fieldValue}
            onChange={handleFieldChange}
            onToggleLock={handleToggleLock}
          />
        )}
        {tab === 'scm' && (
          <ScmTab
            eff={eff}
            prov={prov}
            lockList={lockList}
            fieldValue={fieldValue}
            onChange={handleFieldChange}
            onToggleLock={handleToggleLock}
          />
        )}
        {tab === 'pricing' && (
          <PricingTab
            eff={eff}
            prov={prov}
            lockList={lockList}
            fieldValue={fieldValue}
            onChange={handleFieldChange}
            onToggleLock={handleToggleLock}
          />
        )}
        {tab === 'cp-runtime' && (
          <CpRuntimeTab
            prov={prov}
            lockList={lockList}
            fieldValue={fieldValue}
            onChange={handleFieldChange}
            onToggleLock={handleToggleLock}
            status={configView.status}
          />
        )}
        {tab === 'raw' && (
          <RawTab
            heading={
              <>
                Current <span className="font-mono">~/.rupu/config.toml</span>
              </>
            }
            savedRaw={configView.raw_global}
            draft={rawDraft}
            onChangeDraft={setRawDraft}
            onSave={() => void handleRawSave()}
            saving={rawSaving}
            error={rawError}
            emptyPlaceholder="# empty — no global config.toml written yet\n"
          />
        )}
        {tab === 'policy' && (
          <PolicyTab
            keys={Object.keys(prov).sort()}
            locks={policyDraft ?? lockList}
            onToggle={handlePolicyToggle}
            onSave={() => void handlePolicySave()}
            saving={policySaving}
            error={policySaveError}
            dirty={policyDraft !== null && !sameSet(policyDraft, lockList)}
          />
        )}
        {tab === 'runtime-status' && <RuntimeStatusTab status={configView.status} />}
      </section>
    </div>
  );
}
