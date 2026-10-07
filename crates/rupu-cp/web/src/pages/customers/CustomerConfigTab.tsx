// Customer Config tab — the customer layer's `config.toml` editor. Fetches
// `GET /api/config?customer=<slug>` (effective values merged global+customer,
// per-key provenance, the layer's own `[policy].lock`) and renders the SAME
// General / Providers / Autoflow / SCM-Issues / Pricing tab bodies and Raw tab
// as the global Settings page and the project Config tab
// (components/ConfigEditor.tsx), plus a Policy tab for the layer's locks.
//
// What is different from those two:
//  - A field has three states (ConfigLayerContext in settings/ConfigField.tsx):
//    inherited from global (read-only until "Override for <name>"), owned by
//    the customer (editable, with a "Lock for projects" switch) and pinned by
//    the GLOBAL `[policy].lock` (read-only with the warn chip — the server
//    rejects a write to it with a 400).
//  - A lock toggle writes `patch: {"policy.lock": [...]}` straight away, like
//    Settings' global locks. Lock keys use the canonical dotted-key encoding
//    (`quoteSegment`) — they are the exact strings the field rows carry.
//  - A layer that doesn't parse (`layer_error`) shows a banner and opens the
//    Raw tab, the only way to fix it; `effective` is global-only meanwhile.
//  - Save goes to `PUT /api/config/customer/:slug`; the server's 400 (the layer
//    breaks the merged config, or sets a globally locked key) shows inline.

import { useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { Cpu, DollarSign, FileCode, GitBranch, Shield, SlidersHorizontal, Workflow } from 'lucide-react';
import { api, ApiError, type ConfigView } from '../../lib/api';
import { TabBar, TabButton } from '../../components/TabBar';
import { Button } from '../../components/ui/Button';
import { LayerErrorBanner } from '../../components/settings/LayerErrorBanner';
import {
  ConfigLayerContext,
  FieldGroup,
  LockOwnerChip,
  fieldCls,
  toggleInputCls,
  type ConfigLayerContextValue,
} from '../../components/settings/ConfigField';
import {
  getPath,
  quoteSegment,
  splitDottedKey,
  GeneralTab,
  ProvidersTab,
  AutoflowTab,
  ScmTab,
  PricingTab,
  RawTab,
  EmptyTabState,
} from '../../components/ConfigEditor';

type SubTab = 'general' | 'providers' | 'autoflow' | 'scm' | 'pricing' | 'policy' | 'raw';

export interface CustomerConfigTabProps {
  slug: string;
  name: string;
  projectCount: number;
  /** Shown in the description (`~/.rupu/customers/<slug>/config.toml`). */
  layerPath: string;
  /** Called after a successful write, so the page's own summary (default
   *  account, `layer_error` banner) refetches. */
  onChanged?: () => void;
}

/** Does the customer layer set `[[scm.rules]]`? Rules REPLACE the global ones
 *  (arrays replace, they don't merge), which is worth saying out loud. */
function layerSetsScmRules(view: ConfigView): boolean {
  if (/^\s*\[\[?\s*scm\.rules\s*\]\]?/m.test(view.raw_customer ?? '')) return true;
  return Object.entries(view.provenance).some(
    ([k, v]) => v.source === 'customer' && (k === 'scm.rules' || k.startsWith('scm.rules.')),
  );
}

/** Provider accounts whose name IS the vendor (no `kind` needed). Mirrors
 *  `is_builtin_provider` in `crates/rupu-runtime/src/provider_factory.rs` — the
 *  source of truth; keep the two in lockstep. */
const BUILTIN_VENDORS = new Set([
  'anthropic',
  'openai',
  'openai_codex',
  'codex',
  'gemini',
  'google_gemini',
  'copilot',
  'github_copilot',
  'local',
]);

/** `staged` minus every key `saved` wrote that still holds the saved value. */
export function unstageSaved(
  staged: Record<string, unknown>,
  saved: Record<string, unknown>,
): Record<string, unknown> {
  let next: Record<string, unknown> | null = null;
  for (const [key, value] of Object.entries(saved)) {
    if (!Object.prototype.hasOwnProperty.call(staged, key)) continue;
    if (!sameValue(staged[key], value)) continue;
    next ??= { ...staged };
    delete next[key];
  }
  return next ?? staged;
}

function sameValue(a: unknown, b: unknown): boolean {
  if (Object.is(a, b)) return true;
  try {
    return JSON.stringify(a) === JSON.stringify(b);
  } catch {
    return false;
  }
}

export default function CustomerConfigTab({ slug, name, projectCount, layerPath, onChanged }: CustomerConfigTabProps) {
  const [view, setView] = useState<ConfigView | null>(null);
  // `loadError` is a failed read with nothing to show yet; `reloadError` is a
  // failed re-read while the last good view stays on screen.
  const [loadError, setLoadError] = useState<string | null>(null);
  const [reloadError, setReloadError] = useState<string | null>(null);
  const [pendingPatch, setPendingPatch] = useState<Record<string, unknown>>({});
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);
  const [saveInfo, setSaveInfo] = useState<string | null>(null);
  const [lockError, setLockError] = useState<string | null>(null);
  const [readOnly, setReadOnly] = useState(false);
  const [tab, setTab] = useState<SubTab>('general');
  const [rawSaving, setRawSaving] = useState(false);
  const [rawError, setRawError] = useState<string | null>(null);
  // A broken layer opens Raw — once, on the first load; after that the
  // operator's own tab choice stands.
  const settledTab = useRef(false);
  // Lock writes are read-modify-write of one array, so they are serialised: the
  // ref holds the latest known list (optimistic on submit, re-synced on every
  // successful read) and `lockBusy` keeps a second write out of the PUT→reload
  // window, where it would otherwise drop the first.
  const lockRef = useRef<string[]>([]);
  const lockBusyRef = useRef(false);
  const [lockBusy, setLockBusy] = useState(false);
  // What a save wrote that no successful re-read has confirmed yet. Those keys
  // stay staged until one does (a failed re-read must not make them vanish);
  // then exactly they are un-staged — and only while each still holds the
  // value that was saved, so an edit made during the save, or a field changed
  // again since, stays staged.
  const savedPatchRef = useRef<Record<string, unknown> | null>(null);

  /** Re-read the layer; resolves true on success. A failure keeps the last
   *  good view (and every staged edit) and reports inline. */
  function reload(): Promise<boolean> {
    return api
      .getCustomerConfig(slug)
      .then((data) => {
        setView(data);
        setLoadError(null);
        setReloadError(null);
        lockRef.current = data.customer_lock ?? [];
        const saved = savedPatchRef.current;
        if (saved) {
          savedPatchRef.current = null;
          setPendingPatch((prev) => unstageSaved(prev, saved));
        }
        if (!settledTab.current) {
          settledTab.current = true;
          if (data.layer_error) setTab('raw');
        }
        return true;
      })
      .catch((e: unknown) => {
        const msg = e instanceof Error ? e.message : 'Failed to load customer config';
        setLoadError(msg);
        setReloadError(msg);
        return false;
      });
  }

  useEffect(() => {
    setView(null);
    setPendingPatch({});
    setLoadError(null);
    setReloadError(null);
    settledTab.current = false;
    lockRef.current = [];
    savedPatchRef.current = null;
    void reload();
    // `reload` closes over `slug`; re-fetch only when the customer changes.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [slug]);

  const customerLock = useMemo(() => view?.customer_lock ?? [], [view]);

  function fieldValue(key: string): unknown {
    if (Object.prototype.hasOwnProperty.call(pendingPatch, key)) return pendingPatch[key];
    return view ? getPath(view.effective, key) : undefined;
  }

  function handleFieldChange(key: string, value: unknown) {
    // A globally pinned field renders no input; never stage an edit for it.
    if (view?.provenance[key]?.locked_by === 'global' && view.provenance[key]?.locked) return;
    setSaveInfo(null);
    setPendingPatch((prev) => {
      // Clearing a field (`undefined`) is never staged — the write path has no
      // way to unset a key (only Raw can), so revert to the resolved value.
      if (value === undefined) {
        if (!(key in prev)) return prev;
        const next = { ...prev };
        delete next[key];
        return next;
      }
      return { ...prev, [key]: value };
    });
  }

  function handleOverride(key: string) {
    if (!view) return;
    const current = getPath(view.effective, key);
    if (current === undefined) return;
    setSaveInfo(null);
    setPendingPatch((prev) => ({ ...prev, [key]: current }));
  }

  async function writeLockList(next: string[]) {
    if (lockBusyRef.current) return;
    lockBusyRef.current = true;
    setLockBusy(true);
    setLockError(null);
    setReadOnly(false);
    const prev = lockRef.current;
    lockRef.current = next;
    try {
      await api.putCustomerConfig(slug, { patch: { 'policy.lock': next } });
    } catch (e: unknown) {
      lockRef.current = prev;
      if (e instanceof ApiError && e.status === 501) setReadOnly(true);
      else setLockError(e instanceof Error ? e.message : 'Failed to update the customer lock list');
      lockBusyRef.current = false;
      setLockBusy(false);
      return;
    }
    try {
      await reload();
      onChanged?.();
    } finally {
      lockBusyRef.current = false;
      setLockBusy(false);
    }
  }

  function handleToggleLock(key: string) {
    const cur = lockRef.current;
    void writeLockList(cur.includes(key) ? cur.filter((k) => k !== key) : [...cur, key]);
  }

  async function handleSave() {
    const patch = Object.fromEntries(Object.entries(pendingPatch).filter(([, v]) => v !== undefined));
    if (Object.keys(patch).length === 0) {
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
      await api.putCustomerConfig(slug, { patch });
      // The edits are saved; keep them staged until a re-read confirms, so a
      // transient read failure never makes them vanish from the screen. The
      // re-read here, or a later Retry, un-stages exactly what was saved.
      savedPatchRef.current = { ...(savedPatchRef.current ?? {}), ...patch };
      await reload();
      onChanged?.();
    } catch (e: unknown) {
      if (e instanceof ApiError && e.status === 501) setReadOnly(true);
      else setSaveError(e instanceof Error ? e.message : 'Failed to save customer config');
    } finally {
      setSaving(false);
    }
  }

  async function handleRawSave(draft: string) {
    setRawSaving(true);
    setRawError(null);
    setReadOnly(false);
    try {
      await api.putCustomerConfig(slug, { raw: draft });
      // Staged values were read against the old file; they may be stale now.
      savedPatchRef.current = null;
      setPendingPatch({});
      await reload();
      onChanged?.();
    } catch (e: unknown) {
      if (e instanceof ApiError && e.status === 501) setReadOnly(true);
      else setRawError(e instanceof Error ? e.message : 'Failed to save raw customer config');
      throw e;
    } finally {
      setRawSaving(false);
    }
  }

  const layerBroken = Boolean(view?.layer_error);
  const layer: ConfigLayerContextValue | null = useMemo(() => {
    if (!view) return null;
    return {
      name,
      lockDisabled: lockBusy || layerBroken,
      overrideDisabled: layerBroken,
      disabledReason: layerBroken
        ? "Fix the layer in the Raw tab first — it doesn't parse."
        : 'Saving the lock list…',
      isStaged: (key) => Object.prototype.hasOwnProperty.call(pendingPatch, key),
      onOverride: handleOverride,
      note: (key) => {
        if (key !== 'default_provider' || view.provenance.default_provider?.source !== 'customer') return null;
        const acct = pendingPatch.default_provider ?? getPath(view.effective, 'default_provider');
        if (typeof acct !== 'string' || acct === '') return null;
        const kindKey = `providers.${quoteSegment(acct)}.kind`;
        const kind = getPath(view.effective, kindKey);
        if (typeof kind !== 'string' || kind === '') {
          // A built-in vendor name needs no `kind` — the name is the vendor.
          if (BUILTIN_VENDORS.has(acct)) return null;
          return (
            <p className="mt-1.5 text-note text-warn">
              <code className="font-mono">{acct}</code> isn&apos;t declared — add it with{' '}
              <code className="font-mono">rupu auth login --account {acct} --kind &lt;vendor&gt;</code>
            </p>
          );
        }
        // Where the account's table was actually declared, not assumed.
        const source = view.provenance[kindKey]?.source;
        if (source === undefined) {
          // The kind is set but no layer claims it: say what it is, never claim where.
          return (
            <p className="mt-1.5 text-note text-ink-mute">
              Declared as <code className="font-mono text-ink">{kind}</code>
            </p>
          );
        }
        if (source === 'global') {
          return (
            <p className="mt-1.5 text-note text-ink-mute">
              Declared globally by{' '}
              <code className="font-mono text-ink">
                rupu auth login --account {acct} --kind {kind}
              </code>
            </p>
          );
        }
        const where =
          source === 'customer'
            ? "this customer's layer"
            : source === 'project'
              ? "a project's layer"
              : 'the built-in defaults';
        return (
          <p className="mt-1.5 text-note text-ink-mute">
            Declared in {where} (<code className="font-mono text-ink">kind = &quot;{kind}&quot;</code>)
          </p>
        );
      },
    };
    // handleOverride only reads `view`, which is in the deps.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [view, pendingPatch, name, lockBusy, layerBroken]);

  if (view === null && loadError) {
    return (
      <div
        role="alert"
        className="flex items-center justify-between gap-3 rounded-lg border border-err/30 bg-err-bg px-4 py-3 text-sm text-err"
      >
        <span>{loadError}</span>
        <Button variant="secondary" size="sm" onClick={() => void reload()}>
          Retry
        </Button>
      </div>
    );
  }
  if (view === null || layer === null) {
    return <div className="text-sm text-ink-dim">Loading customer config…</div>;
  }

  const eff = view.effective;
  const prov = view.provenance;
  const dirtyCount = Object.keys(pendingPatch).length;
  const tabProps = {
    eff,
    prov,
    lockList: customerLock,
    fieldValue,
    onChange: handleFieldChange,
    onToggleLock: handleToggleLock,
  };

  return (
    <div className="space-y-6">
      <div className="flex items-start justify-between gap-4">
        <p className="text-sm text-ink-dim">
          Customer layer, resolved from <span className="font-mono">{layerPath}</span>. It applies to {name}&apos;s{' '}
          {projectCount} project{projectCount === 1 ? '' : 's'}, between the global config and each repo&apos;s{' '}
          <span className="font-mono">.rupu/config.toml</span>. A key you lock here can&apos;t be overridden by the
          repo.
        </p>
        <div className="flex shrink-0 items-center gap-3">
          {dirtyCount > 0 && (
            <span className="text-note text-ink-dim">
              {dirtyCount} unsaved change{dirtyCount === 1 ? '' : 's'}
            </span>
          )}
          <Button onClick={() => void handleSave()} disabled={dirtyCount === 0 || saving || Boolean(view.layer_error)}>
            {saving ? 'Saving…' : 'Save changes'}
          </Button>
        </div>
      </div>

      {reloadError && (
        <div
          role="alert"
          className="flex items-center justify-between gap-3 rounded-lg border border-err/30 bg-err-bg px-4 py-3 text-sm text-err"
        >
          <span>Couldn&apos;t refresh this layer: {reloadError}</span>
          <Button variant="secondary" size="sm" onClick={() => void reload()}>
            Retry
          </Button>
        </div>
      )}
      {view.layer_error && (
        <LayerErrorBanner message={view.layer_error} hint="Fix it in the Raw tab — the form can't edit a file that doesn't parse." />
      )}
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
      {lockError && (
        <div role="alert" className="rounded-lg border border-err/30 bg-err-bg px-4 py-3 text-sm text-err">
          {lockError}
        </div>
      )}
      {saveInfo && (
        <div className="rounded-lg border border-border bg-surface px-4 py-3 text-sm text-ink-dim">{saveInfo}</div>
      )}

      <TabBar>
        <TabButton active={tab === 'general'} onClick={() => setTab('general')} icon={SlidersHorizontal} label="General" />
        <TabButton active={tab === 'providers'} onClick={() => setTab('providers')} icon={Cpu} label="Providers" />
        <TabButton active={tab === 'autoflow'} onClick={() => setTab('autoflow')} icon={Workflow} label="Autoflow" />
        <TabButton active={tab === 'scm'} onClick={() => setTab('scm')} icon={GitBranch} label="SCM / Issues" />
        <TabButton active={tab === 'pricing'} onClick={() => setTab('pricing')} icon={DollarSign} label="Pricing" />
        <TabButton active={tab === 'policy'} onClick={() => setTab('policy')} icon={Shield} label="Policy" />
        <TabButton active={tab === 'raw'} onClick={() => setTab('raw')} icon={FileCode} label="Raw" />
      </TabBar>

      <ConfigLayerContext.Provider value={layer}>
        <section className="bg-panel border border-border rounded-xl shadow-card px-5 py-4">
          {tab === 'general' && <GeneralTab {...tabProps} />}
          {tab === 'providers' && <ProvidersTab {...tabProps} />}
          {tab === 'autoflow' && <AutoflowTab {...tabProps} />}
          {tab === 'scm' && (
            <div className="space-y-4">
              {layerSetsScmRules(view) && (
                <div className="rounded-lg border border-warn/30 bg-warn-bg px-4 py-3 text-sm text-warn">
                  These rules replace the global [[scm.rules]] for {name}&apos;s projects (arrays replace, they
                  don&apos;t merge).
                </div>
              )}
              <ScmTab {...tabProps} />
            </div>
          )}
          {tab === 'pricing' && <PricingTab {...tabProps} />}
          {tab === 'policy' && (
            <PolicyTab
              name={name}
              customerLock={customerLock}
              globalLock={Object.keys(prov).filter((k) => prov[k]?.locked && prov[k]?.locked_by === 'global')}
              onToggle={handleToggleLock}
              onAdd={(key) => {
                if (!lockRef.current.includes(key)) void writeLockList([...lockRef.current, key]);
              }}
              disabled={lockBusy || layerBroken}
              disabledReason={layerBroken ? "Fix the layer in the Raw tab first — it doesn't parse." : undefined}
            />
          )}
          {tab === 'raw' && (
            <RawTab
              heading={
                <>
                  Current <span className="font-mono">{layerPath}</span> (this customer)
                </>
              }
              savedRaw={view.raw_customer ?? ''}
              onSave={handleRawSave}
              saving={rawSaving}
              saveError={rawError}
              emptyPlaceholder="# empty — no customer config.toml written yet\n"
            />
          )}
        </section>
      </ConfigLayerContext.Provider>

      <p className="text-note text-ink-mute">
        Prefer a terminal? Edit and validate the layer with{' '}
        <code className="font-mono text-ink">rupu customer edit {slug}</code>; see the effective config with sources
        and locks with <code className="font-mono text-ink">rupu customer show {slug}</code>.
      </p>
    </div>
  );
}

/** The layer's `[policy].lock`: what projects can't override, plus (read-only)
 *  what the global policy already pins for everyone. */
function PolicyTab({
  name,
  customerLock,
  globalLock,
  onToggle,
  onAdd,
  disabled,
  disabledReason,
}: {
  name: string;
  customerLock: string[];
  globalLock: string[];
  onToggle: (key: string) => void;
  onAdd: (key: string) => void;
  disabled: boolean;
  disabledReason?: string;
}): ReactNode {
  const [draft, setDraft] = useState('');
  const key = draft.trim();
  const keyOk = key === '' || isCanonicalKey(key);
  return (
    <div className="space-y-4">
      <FieldGroup
        title={`Locked for ${name}'s projects`}
        description="A key locked here can't be overridden by a project's own .rupu/config.toml."
      >
        {customerLock.length === 0 ? (
          <EmptyTabState text="No keys locked by this customer yet. Use “Lock for projects” on a field, or add a key below." />
        ) : (
          customerLock.map((k) => (
            <div key={k} className="flex items-center gap-3 py-2 first:pt-0 last:pb-0">
              <input
                id={`customer-lock-${k}`}
                type="checkbox"
                role="switch"
                checked
                disabled={disabled}
                title={disabled ? disabledReason : undefined}
                onChange={() => onToggle(k)}
                className={toggleInputCls}
              />
              <label htmlFor={`customer-lock-${k}`} className="min-w-0 flex-1 truncate font-mono text-sm text-ink">
                {k}
              </label>
              <LockOwnerChip owner="customer" />
            </div>
          ))
        )}
        <form
          className="flex items-center gap-2 pt-3"
          onSubmit={(e) => {
            e.preventDefault();
            if (key && keyOk && !disabled) {
              onAdd(key);
              setDraft('');
            }
          }}
        >
          <input
            aria-label="Key to lock"
            aria-invalid={!keyOk}
            aria-describedby={keyOk ? undefined : 'customer-lock-key-help'}
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            placeholder="providers.anthropic.default_model"
            disabled={disabled}
            className={fieldCls}
          />
          <Button type="submit" variant="secondary" disabled={key === '' || !keyOk || disabled}>
            Lock key
          </Button>
        </form>
        {!keyOk && (
          <p id="customer-lock-key-help" className="pt-1 text-note text-err">
            Not a canonical key. Quote any segment that contains a dot, e.g.{' '}
            <span className="font-mono">providers.&quot;azure.eastus&quot;.base_url</span>.
          </p>
        )}
        {disabled && disabledReason && <p className="pt-1 text-note text-ink-mute">{disabledReason}</p>}
      </FieldGroup>

      {globalLock.length > 0 && (
        <FieldGroup
          title="Pinned by the global policy"
          description="The global [policy].lock pins these — a customer can't change them."
        >
          {globalLock.map((k) => (
            <div key={k} className="flex items-center gap-3 py-2 first:pt-0 last:pb-0">
              <span className="min-w-0 flex-1 truncate font-mono text-sm text-ink">{k}</span>
              <LockOwnerChip owner="global" />
            </div>
          ))}
        </FieldGroup>
      )}
    </div>
  );
}

/** A lock key must be the exact string the field rows carry: every segment
 *  non-empty and joined with `quoteSegment`, so a dotted segment is quoted. */
function isCanonicalKey(key: string): boolean {
  const segs = splitDottedKey(key);
  return segs.every((s) => s !== '') && segs.map(quoteSegment).join('.') === key;
}
