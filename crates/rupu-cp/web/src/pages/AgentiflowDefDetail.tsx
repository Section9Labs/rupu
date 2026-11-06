// Agentiflow definition page — /agentiflows/defs/:name (operator MCP Plan 3b,
// UI-2). The workflow editor's layout without the graph (a definition is
// goals, a budget and a pool, not steps): a header with the name, its layer,
// the server's validity badge and Save / Revert / Delete / Launch, above the
// YAML pane. `?scope_kind=&scope_id=` pins the layer's file; without them the
// server reads the project-first match. Save and Delete act on the file the
// page loaded.

import { useEffect, useMemo, useState } from 'react';
import { useNavigate, useParams, useSearchParams } from 'react-router-dom';
import { Trash2 } from 'lucide-react';
import { apiErrorMessage } from '../lib/api';
import { agentiflowsApi, defSelectorFor, type AgentiflowDefDetail } from '../lib/api3b/agentiflows';
import { leadAndProfiles, selectorFromSearch, useDefValidity } from '../lib/agentiflowDefs';
import CodeEditor from '../components/CodeEditor';
import AgentiflowLauncherSheet from '../components/agentiflow/AgentiflowLauncherSheet';
import { ConfirmCard } from '../components/agentiflow/ConfirmCard';
import { DefValidityBadge } from '../components/agentiflow/DefValidityBadge';
import { Button } from '../components/ui/Button';
import { ErrorBanner } from '../components/ui/ErrorBanner';
import { PageHeader } from '../components/ui/PageHeader';
import { Spinner } from '../components/ui/Spinner';
import { DefLayer } from './AgentiflowDefs';

const LIST = '/agentiflows/defs';
const BACK = { to: LIST, label: 'Agentiflows' };

function asString(v: unknown): string | undefined {
  return typeof v === 'string' ? v : undefined;
}

function asStrings(v: unknown): string[] {
  return Array.isArray(v) ? v.filter((x): x is string => typeof x === 'string') : [];
}

export default function AgentiflowDefDetailPage() {
  const { name = '' } = useParams<{ name: string }>();
  const [search] = useSearchParams();
  const navigate = useNavigate();
  const selector = useMemo(() => selectorFromSearch(search), [search]);

  const [detail, setDetail] = useState<AgentiflowDefDetail | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [draft, setDraft] = useState('');
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [deleting, setDeleting] = useState(false);
  const [deleteError, setDeleteError] = useState<string | null>(null);
  const [launcherOpen, setLauncherOpen] = useState(false);

  useEffect(() => {
    let live = true;
    setDetail(null);
    setLoadError(null);
    agentiflowsApi
      .getDef(name, selector)
      .then((d) => {
        if (!live) return;
        setDetail(d);
        setDraft(d.yaml);
      })
      .catch((e: unknown) => {
        if (live) setLoadError(apiErrorMessage(e));
      });
    return () => {
      live = false;
    };
  }, [name, selector]);

  // Checked against the project's own engagement profiles for a project file.
  const validity = useDefValidity(draft, detail?.scope_kind === 'project' ? detail.scope_id : undefined);

  const dirty = detail !== null && draft !== detail.yaml;
  useEffect(() => {
    if (!dirty) return;
    const handler = (e: BeforeUnloadEvent) => {
      e.preventDefault();
      e.returnValue = '';
    };
    window.addEventListener('beforeunload', handler);
    return () => window.removeEventListener('beforeunload', handler);
  }, [dirty]);

  if (loadError) {
    return (
      <div className="p-8">
        <PageHeader title={name} back={BACK} />
        <ErrorBanner className="mt-4">{loadError}</ErrorBanner>
      </div>
    );
  }

  if (!detail) {
    return (
      <div className="p-8">
        <PageHeader title={name} back={BACK} />
        <div className="py-12 flex items-center justify-center">
          <Spinner label="Loading definition…" />
        </div>
      </div>
    );
  }

  const loaded = detail;
  const sel = defSelectorFor(loaded);
  const layerLabel = loaded.scope_kind === 'project' ? `the ${loaded.scope} project` : 'the global layer';

  async function save() {
    if (saving) return;
    setSaving(true);
    setSaveError(null);
    try {
      const updated = await agentiflowsApi.saveDef(name, draft, sel);
      setDetail(updated);
      setDraft(updated.yaml);
    } catch (e: unknown) {
      setSaveError(apiErrorMessage(e));
    } finally {
      setSaving(false);
    }
  }

  async function remove() {
    if (deleting) return;
    setDeleting(true);
    setDeleteError(null);
    try {
      const res = await agentiflowsApi.deleteDef(name, sel);
      if (res.scope_kind !== loaded.scope_kind || res.scope !== loaded.scope) {
        // Never read a bare "deleted" as "deleted what I saw".
        setDeleteError(
          `Deleted the ${res.scope_kind === 'project' ? `project (${res.scope})` : 'global'} definition, not the one shown here.`,
        );
        setDeleting(false);
        setConfirmDelete(false);
        return;
      }
      navigate(LIST);
    } catch (e: unknown) {
      setDeleteError(apiErrorMessage(e));
      setDeleting(false);
      setConfirmDelete(false);
    }
  }

  const def = loaded.def ?? {};
  const description = asString(def.description);
  const summary = validity?.ok
    ? leadAndProfiles(validity.lead, validity.engagement_profiles)
    : loaded.def
      ? leadAndProfiles(asString(def.lead) ?? null, asStrings(def.engagement_profiles))
      : null;

  return (
    <div className="p-8">
      <PageHeader
        back={BACK}
        title={asString(def.name) ?? name}
        subtitle={
          <span className="inline-flex flex-wrap items-center gap-2">
            <DefLayer def={loaded} />
            {description}
          </span>
        }
        actions={
          <>
            <DefValidityBadge validity={validity} />
            <Button variant="secondary" onClick={() => setDraft(loaded.yaml)} disabled={saving || !dirty}>
              Revert
            </Button>
            <Button variant="solid" onClick={() => void save()} disabled={saving || !dirty || validity?.ok === false}>
              {saving ? 'Saving…' : 'Save'}
            </Button>
            <Button
              variant="danger-outline"
              onClick={() => setConfirmDelete(true)}
              disabled={deleting || confirmDelete}
              className="gap-1.5"
            >
              <Trash2 size={14} />
              Delete
            </Button>
            <Button variant="secondary" onClick={() => setLauncherOpen(true)}>
              Launch
            </Button>
          </>
        }
      />

      <div className="mt-3 max-w-3xl space-y-2">
        {confirmDelete && (
          <ConfirmCard
            title={`Delete ${name}?`}
            cancelLabel="Keep it"
            confirmLabel="Delete"
            busyLabel="Deleting…"
            busy={deleting}
            onCancel={() => setConfirmDelete(false)}
            onConfirm={() => void remove()}
          >
            This removes the definition file from {layerLabel}. Past runs keep their own copy. This cannot be undone.
          </ConfirmCard>
        )}
        {saveError && (
          <p role="alert" className="text-ui font-medium text-err">
            {saveError}
          </p>
        )}
        {deleteError && (
          <p role="alert" className="text-ui font-medium text-err">
            {deleteError}
          </p>
        )}
        {loaded.error && <p className="break-words font-mono text-note text-err">{loaded.error}</p>}
        {summary && <p className="text-note text-ink-dim">Lead · profiles: {summary}</p>}
      </div>

      <div className="mt-5">
        <CodeEditor value={draft} onChange={setDraft} language="yaml" ariaLabel="Agentiflow definition" />
        {validity && !validity.ok && <p className="mt-2 break-words font-mono text-note text-err">{validity.error}</p>}
      </div>

      {launcherOpen && (
        <AgentiflowLauncherSheet
          initial={{ name, scope_kind: loaded.scope_kind, scope_id: loaded.scope_id }}
          onClose={() => setLauncherOpen(false)}
        />
      )}
    </div>
  );
}
