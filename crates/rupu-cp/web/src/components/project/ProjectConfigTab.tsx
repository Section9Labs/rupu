// Project Config tab — per-project `.rupu/config.toml` editor (T6). Fetches
// the project-resolved `GET /api/config?project=<id>` view (effective values
// merged global+project, plus per-key provenance) and renders the SAME
// General / Providers / Autoflow / SCM-Issues / Pricing / CP field tabs +
// Raw tab as the global Settings page (components/ConfigEditor.tsx),
// parameterized by `wsId`.
//
// Differences from the global Settings form:
//  - A key already enforced by the GLOBAL `[policy].lock` list renders
//    read-only here (🔒 + "enforced by global policy" note, no input, no
//    lock toggle) — locking is a global-only concept the project layer
//    cannot set or override.
//  - No Policy or Runtime-status tabs (both are global-only concepts).
//  - Save posts to `PUT /api/config/project/:id`, which additionally REJECTS
//    (400) a patch that would set a locked key — surfaced inline like any
//    other validation error.
//  - The Raw tab edits `raw_project` (this workspace's own file), not the
//    merged/global text.

import { useEffect, useState } from 'react';
import { Cpu, DollarSign, FileCode, GitBranch, Server, SlidersHorizontal, Workflow } from 'lucide-react';
import { api, ApiError, type ConfigView } from '../../lib/api';
import { TabBar, TabButton } from '../TabBar';
import { Button } from '../ui/Button';
import { getPath, GeneralTab, ProvidersTab, AutoflowTab, ScmTab, PricingTab, CpFieldTab, RawTab } from '../ConfigEditor';

type ProjectConfigSubTab = 'general' | 'providers' | 'autoflow' | 'scm' | 'pricing' | 'cp' | 'raw';

export default function ProjectConfigTab({ wsId }: { wsId: string }) {
  const [configView, setConfigView] = useState<ConfigView | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [pendingPatch, setPendingPatch] = useState<Record<string, unknown>>({});
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);
  const [saveInfo, setSaveInfo] = useState<string | null>(null);
  const [readOnly, setReadOnly] = useState(false);
  const [tab, setTab] = useState<ProjectConfigSubTab>('general');

  // ── Raw TOML tab state ──────────────────────────────────────────────────
  // The draft itself lives inside `RawTab` (self-contained edit-mode), seeded
  // from `configView.raw_project` only when the operator clicks Edit — so an
  // unrelated reload (e.g. a form-tab Save) can't clobber an in-flight edit.
  const [rawSaving, setRawSaving] = useState(false);
  const [rawError, setRawError] = useState<string | null>(null);

  function reload(): Promise<void> {
    return api
      .getConfig(wsId)
      .then((data) => {
        setConfigView(data);
        setLoadError(null);
      })
      .catch((e: unknown) => {
        setLoadError(e instanceof Error ? e.message : 'Failed to load project config');
      });
  }

  useEffect(() => {
    setConfigView(null);
    setPendingPatch({});
    void reload();
    // reload() closes over `wsId` freshly each render; only re-run the fetch
    // when the project itself changes.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [wsId]);

  // Keys enforced by the GLOBAL policy lock — derived straight from
  // provenance (no local toggle state to keep in sync; this view can't set
  // or clear the lock, only render around it).
  const lockList = configView
    ? Object.keys(configView.provenance).filter((k) => configView.provenance[k]?.locked)
    : [];

  function fieldValue(key: string): unknown {
    if (Object.prototype.hasOwnProperty.call(pendingPatch, key)) return pendingPatch[key];
    return configView ? getPath(configView.effective, key) : undefined;
  }

  function handleFieldChange(key: string, value: unknown) {
    // A locked field never renders an editable control, so this shouldn't
    // fire for one — but never stage an edit for it regardless.
    if (lockList.includes(key)) return;
    setSaveInfo(null);
    setPendingPatch((prev) => {
      // Clearing a field yields `undefined`. As with the global form, an
      // `undefined` transition is never staged as an edit (it would
      // otherwise produce a silently-dropped, no-op patch on Save) — drop
      // the key from the pending patch instead, reverting the displayed
      // value back to its current resolved value.
      if (value === undefined) {
        if (!(key in prev)) return prev;
        const next = { ...prev };
        delete next[key];
        return next;
      }
      return { ...prev, [key]: value };
    });
  }

  async function handleSave() {
    if (Object.keys(pendingPatch).length === 0) return;
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
      await api.putProjectConfig(wsId, { patch: effectivePatch });
      setPendingPatch({});
      await reload();
    } catch (e: unknown) {
      if (e instanceof ApiError && e.status === 501) {
        setReadOnly(true);
      } else {
        setSaveError(e instanceof Error ? e.message : 'Failed to save project config');
      }
    } finally {
      setSaving(false);
    }
  }

  async function handleRawSave(draft: string) {
    setRawSaving(true);
    setRawError(null);
    setReadOnly(false);
    try {
      await api.putProjectConfig(wsId, { raw: draft });
      await reload();
    } catch (e: unknown) {
      if (e instanceof ApiError && e.status === 501) {
        setReadOnly(true);
      } else {
        setRawError(e instanceof Error ? e.message : 'Failed to save raw project config');
      }
      throw e;
    } finally {
      setRawSaving(false);
    }
  }

  if (loadError) {
    return (
      <div role="alert" className="rounded-lg border border-err/30 bg-err-bg px-4 py-3 text-sm text-err">
        {loadError}
      </div>
    );
  }

  if (configView === null) {
    return <div className="text-sm text-ink-dim">Loading project config…</div>;
  }

  const eff = configView.effective;
  const prov = configView.provenance;
  const dirtyCount = Object.keys(pendingPatch).length;

  return (
    <div className="space-y-6">
      <div className="flex items-start justify-between gap-4">
        <p className="text-sm text-ink-dim">
          Project-level overrides, resolved from <span className="font-mono">.rupu/config.toml</span> in
          this workspace. Fields already enforced by the global policy lock are read-only here.
        </p>
        <div className="flex shrink-0 items-center gap-3">
          {dirtyCount > 0 && (
            <span className="text-note text-ink-dim">
              {dirtyCount} unsaved change{dirtyCount === 1 ? '' : 's'}
            </span>
          )}
          <Button onClick={() => void handleSave()} disabled={dirtyCount === 0 || saving}>
            {saving ? 'Saving…' : 'Save changes'}
          </Button>
        </div>
      </div>

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

      <TabBar>
        <TabButton active={tab === 'general'} onClick={() => setTab('general')} icon={SlidersHorizontal} label="General" />
        <TabButton active={tab === 'providers'} onClick={() => setTab('providers')} icon={Cpu} label="Providers" />
        <TabButton active={tab === 'autoflow'} onClick={() => setTab('autoflow')} icon={Workflow} label="Autoflow" />
        <TabButton active={tab === 'scm'} onClick={() => setTab('scm')} icon={GitBranch} label="SCM / Issues" />
        <TabButton active={tab === 'pricing'} onClick={() => setTab('pricing')} icon={DollarSign} label="Pricing" />
        <TabButton active={tab === 'cp'} onClick={() => setTab('cp')} icon={Server} label="CP" />
        <TabButton active={tab === 'raw'} onClick={() => setTab('raw')} icon={FileCode} label="Raw" />
      </TabBar>

      <section className="bg-panel border border-border rounded-xl shadow-card px-5 py-4">
        {tab === 'general' && (
          <GeneralTab
            eff={eff}
            prov={prov}
            lockList={lockList}
            fieldValue={fieldValue}
            onChange={handleFieldChange}
            lockedReadOnly
          />
        )}
        {tab === 'providers' && (
          <ProvidersTab
            eff={eff}
            prov={prov}
            lockList={lockList}
            fieldValue={fieldValue}
            onChange={handleFieldChange}
            lockedReadOnly
          />
        )}
        {tab === 'autoflow' && (
          <AutoflowTab
            eff={eff}
            prov={prov}
            lockList={lockList}
            fieldValue={fieldValue}
            onChange={handleFieldChange}
            lockedReadOnly
          />
        )}
        {tab === 'scm' && (
          <ScmTab
            eff={eff}
            prov={prov}
            lockList={lockList}
            fieldValue={fieldValue}
            onChange={handleFieldChange}
            lockedReadOnly
          />
        )}
        {tab === 'pricing' && (
          <PricingTab
            eff={eff}
            prov={prov}
            lockList={lockList}
            fieldValue={fieldValue}
            onChange={handleFieldChange}
            lockedReadOnly
          />
        )}
        {tab === 'cp' && (
          <CpFieldTab
            prov={prov}
            lockList={lockList}
            fieldValue={fieldValue}
            onChange={handleFieldChange}
            lockedReadOnly
          />
        )}
        {tab === 'raw' && (
          <RawTab
            heading={
              <>
                Current <span className="font-mono">.rupu/config.toml</span> (this project)
              </>
            }
            savedRaw={configView.raw_project ?? ''}
            onSave={handleRawSave}
            saving={rawSaving}
            saveError={rawError}
            emptyPlaceholder="# empty — no project config.toml written yet\n"
          />
        )}
      </section>
    </div>
  );
}
