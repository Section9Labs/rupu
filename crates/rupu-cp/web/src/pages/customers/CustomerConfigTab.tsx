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

export default function CustomerConfigTab({ slug, name, projectCount, layerPath, onChanged }: CustomerConfigTabProps) {
  const [view, setView] = useState<ConfigView | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
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

  function reload(): Promise<void> {
    return api
      .getCustomerConfig(slug)
      .then((data) => {
        setView(data);
        setLoadError(null);
        if (!settledTab.current) {
          settledTab.current = true;
          if (data.layer_error) setTab('raw');
        }
      })
      .catch((e: unknown) => {
        setLoadError(e instanceof Error ? e.message : 'Failed to load customer config');
      });
  }

  useEffect(() => {
    setView(null);
    setPendingPatch({});
    settledTab.current = false;
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
    setLockError(null);
    setReadOnly(false);
    try {
      await api.putCustomerConfig(slug, { patch: { 'policy.lock': next } });
      await reload();
      onChanged?.();
    } catch (e: unknown) {
      if (e instanceof ApiError && e.status === 501) setReadOnly(true);
      else setLockError(e instanceof Error ? e.message : 'Failed to update the customer lock list');
    }
  }

  function handleToggleLock(key: string) {
    void writeLockList(customerLock.includes(key) ? customerLock.filter((k) => k !== key) : [...customerLock, key]);
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
      setPendingPatch({});
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

  const layer: ConfigLayerContextValue | null = useMemo(() => {
    if (!view) return null;
    return {
      name,
      isStaged: (key) => Object.prototype.hasOwnProperty.call(pendingPatch, key),
      onOverride: handleOverride,
      note: (key) => {
        if (key !== 'default_provider' || view.provenance.default_provider?.source !== 'customer') return null;
        const acct = pendingPatch.default_provider ?? getPath(view.effective, 'default_provider');
        if (typeof acct !== 'string' || acct === '') return null;
        const kind = getPath(view.effective, `providers.${quoteSegment(acct)}.kind`);
        if (typeof kind !== 'string' || kind === '') return null;
        return (
          <p className="mt-1.5 text-note text-ink-mute">
            Declared globally by{' '}
            <code className="font-mono text-ink">
              rupu auth login --account {acct} --kind {kind}
            </code>
          </p>
        );
      },
    };
    // handleOverride only reads `view`, which is in the deps.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [view, pendingPatch, name]);

  if (loadError) {
    return (
      <div role="alert" className="rounded-lg border border-err/30 bg-err-bg px-4 py-3 text-sm text-err">
        {loadError}
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
                if (!customerLock.includes(key)) void writeLockList([...customerLock, key]);
              }}
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
}: {
  name: string;
  customerLock: string[];
  globalLock: string[];
  onToggle: (key: string) => void;
  onAdd: (key: string) => void;
}): ReactNode {
  const [draft, setDraft] = useState('');
  const key = draft.trim();
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
            if (key) {
              onAdd(key);
              setDraft('');
            }
          }}
        >
          <input
            aria-label="Key to lock"
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            placeholder="providers.anthropic.default_model"
            className={fieldCls}
          />
          <Button type="submit" variant="secondary" disabled={key === ''}>
            Lock key
          </Button>
        </form>
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
