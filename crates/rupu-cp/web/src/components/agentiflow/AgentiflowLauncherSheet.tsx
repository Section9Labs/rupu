// AgentiflowLauncherSheet — "New agentiflow" (operator MCP Plan 3b, UI-2,
// mock §4): pick a definition and where the flow runs, then Launch
// (`POST /api/agentiflows`) and open the new flow. A flow is local-only, so
// there is no host picker.
//
// Where it runs: the layer the definition lives in (the default), one of the
// registered projects, or an absolute directory. The server looks the
// definition up from there, project first, then global.

import { useEffect, useMemo, useState } from 'react';
import { Link, useNavigate } from 'react-router-dom';
import { api, apiErrorMessage, type ProjectRow } from '../../lib/api';
import { agentiflowsApi, type AgentiflowDefRow, type DefScopeKind, type LaunchScope } from '../../lib/api3b/agentiflows';
import { leadAndProfiles } from '../../lib/agentiflowDefs';
import { DialogFrame } from '../customers/DialogFrame';
import { Button } from '../ui/Button';
import { Field } from '../ui/Field';
import { Input } from '../ui/Input';
import { Select } from '../ui/Select';

/** The definition the sheet opens on (a Library row's or a definition page's). */
export interface LaunchInitial {
  name: string;
  scope_kind: DefScopeKind;
  scope_id: string | null;
}

const keyOf = (d: { name: string; scope_kind: DefScopeKind; scope_id: string | null }) =>
  `${d.scope_kind}:${d.scope_id ?? ''}:${d.name}`;

type Started = { id: string; log?: string };

export default function AgentiflowLauncherSheet({
  initial,
  onClose,
}: {
  initial?: LaunchInitial;
  onClose: () => void;
}) {
  const navigate = useNavigate();
  const [defs, setDefs] = useState<AgentiflowDefRow[] | null>(null);
  const [projects, setProjects] = useState<ProjectRow[]>([]);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [defKey, setDefKey] = useState(initial ? keyOf(initial) : '');
  const [where, setWhere] = useState('defined');
  const [dir, setDir] = useState('');
  const [launching, setLaunching] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [pending, setPending] = useState<Started | null>(null);

  useEffect(() => {
    let live = true;
    agentiflowsApi
      .listDefs()
      .then((rows) => {
        if (!live) return;
        setDefs(rows);
        const first = rows.find((r) => !r.error) ?? rows[0];
        setDefKey((k) => (k && rows.some((r) => keyOf(r) === k) ? k : first ? keyOf(first) : ''));
      })
      .catch((e: unknown) => {
        if (live) setLoadError(apiErrorMessage(e));
      });
    // Projects only widen where it can run; the sheet works without them.
    api
      .getProjects()
      .then((p) => {
        if (live) setProjects(p);
      })
      .catch(() => {});
    return () => {
      live = false;
    };
  }, []);

  const def = useMemo(() => defs?.find((d) => keyOf(d) === defKey) ?? null, [defs, defKey]);

  const scope: LaunchScope | null = useMemo(() => {
    if (!def) return null;
    if (where === 'defined') return { scope_kind: def.scope_kind, scope_id: def.scope_id };
    if (where === 'dir') return dir.trim().startsWith('/') ? { working_dir: dir.trim() } : null;
    const project = projects.find((p) => `project:${p.ws_id}` === where);
    return project ? { working_dir: project.path } : null;
  }, [def, where, dir, projects]);

  async function launch() {
    if (!def || !scope || launching) return;
    setLaunching(true);
    setError(null);
    try {
      const res = await agentiflowsApi.launch(def.name, scope);
      if (res.status === 'pending') {
        setPending({ id: res.id, log: res.log });
      } else {
        navigate(`/agentiflows/${encodeURIComponent(res.id)}`);
        onClose();
      }
    } catch (e: unknown) {
      setError(apiErrorMessage(e));
    } finally {
      setLaunching(false);
    }
  }

  const definedLabel = def ? (def.scope_kind === 'project' ? `its project · ${def.scope}` : 'the global layer') : '—';
  const busy = launching || pending !== null;

  return (
    <DialogFrame title="New agentiflow run" onRequestClose={onClose} className="max-w-lg">
      <p className="mt-1 text-ui text-ink-dim">Start a flow on this machine. It keeps running when you close the page.</p>

      <div className="mt-3">
        <Field label="Definition" help={def ? leadAndProfiles(def.lead, def.engagement_profiles) : undefined}>
          {defs !== null && defs.length === 0 ? (
            <p className="text-ui text-ink-dim">
              No agentiflow definitions yet. Add one in Library → Agentiflows.
            </p>
          ) : (
            <Select
              aria-label="Definition"
              className="w-full"
              value={defKey}
              onChange={(e) => setDefKey(e.target.value)}
              disabled={busy || defs === null}
            >
              {defs === null && <option value="">Loading…</option>}
              {(defs ?? []).map((d) => (
                <option key={keyOf(d)} value={keyOf(d)} disabled={d.error !== null}>
                  {d.name} · {d.scope_kind === 'project' ? d.scope : 'global'}
                  {d.error ? ' (does not parse)' : ''}
                </option>
              ))}
            </Select>
          )}
        </Field>

        <Field label="Runs in" help="The definition is looked up from there, project first, then global.">
          <Select
            aria-label="Runs in"
            className="w-full"
            value={where}
            onChange={(e) => setWhere(e.target.value)}
            disabled={busy || !def}
          >
            <option value="defined">Where it's defined: {definedLabel}</option>
            {projects.map((p) => (
              <option key={p.ws_id} value={`project:${p.ws_id}`}>
                Project: {p.name}
              </option>
            ))}
            <option value="dir">A directory…</option>
          </Select>
          {where === 'dir' && (
            <Input
              aria-label="Directory"
              className="mt-2 max-w-none font-mono"
              placeholder="/absolute/path"
              value={dir}
              onChange={(e) => setDir(e.target.value)}
              disabled={busy}
            />
          )}
        </Field>
      </div>

      {loadError && (
        <p role="alert" className="mt-2 text-ui font-medium text-err">
          {loadError}
        </p>
      )}
      {error && (
        <p role="alert" className="mt-2 text-ui font-medium text-err">
          {error}
        </p>
      )}
      {pending && (
        <div role="status" className="mt-2 rounded-lg border border-info/30 bg-info-bg px-3 py-2 text-ui text-info">
          <p>
            Still starting after 30 s.{' '}
            <Link to={`/agentiflows/${encodeURIComponent(pending.id)}`} className="font-medium underline" onClick={onClose}>
              Open {pending.id} →
            </Link>
          </p>
          {pending.log && (
            <pre className="mt-1.5 max-h-40 overflow-auto whitespace-pre-wrap font-mono text-note text-ink-dim">
              {pending.log}
            </pre>
          )}
        </div>
      )}

      <div className="mt-4 flex items-center justify-end gap-2">
        <Button variant="secondary" onClick={onClose} disabled={launching}>
          {pending ? 'Close' : 'Cancel'}
        </Button>
        <Button variant="solid" onClick={() => void launch()} disabled={busy || !def || def.error !== null || !scope}>
          {launching ? 'Launching…' : 'Launch'}
        </Button>
      </div>
    </DialogFrame>
  );
}
