// InstallSkillsDialog — Library → Skills' "Install skills" (Plan 3b mock §6).
// A source (an absolute directory, owner/repo or an https git URL), a ref
// and a layer; "Find skills" runs the install's dry run, which lists the
// source's skills with what installing each would do. New ones are ticked.
// Replacing an installed copy needs "Replace locally edited copies" (the
// install's `force`), and installing one asks first: the copy's local edits
// are lost. After an install the dialog runs the dry run again, so
// what it shows as installed is what the server now reports, never the POST
// alone. Rendered by its owner only while open.

import { useId, useState, type FormEvent } from 'react';
import { apiErrorMessage, type ProjectRow } from '../../lib/api';
import {
  installSkills,
  previewSkillInstall,
  type SkillInstallPreview,
  type SkillInstallReport,
  type SkillPreviewRow,
  type SkillPreviewStatus,
} from '../../lib/api3b/library';
import { DialogFrame } from '../customers/DialogFrame';
import { Button } from '../ui/Button';
import { Input } from '../ui/Input';
import { Select } from '../ui/Select';
import { StatusChip, type ChipTone } from '../ui/StatusChip';
import { cn } from '../../lib/cn';

const STATUS: Record<SkillPreviewStatus, { label: string; tone: ChipTone }> = {
  new: { label: 'new', tone: 'neutral' },
  up_to_date: { label: 'up to date', tone: 'completed' },
  changed: { label: 'update available', tone: 'running' },
  locally_modified: { label: 'edited locally', tone: 'awaiting_approval' },
  invalid: { label: 'invalid', tone: 'failed' },
};

/** Installing it replaces an installed copy: only with `force`. */
const needsForce = (s: SkillPreviewStatus) => s === 'changed' || s === 'locally_modified';
const installable = (r: SkillPreviewRow, force: boolean) => r.status === 'new' || (force && needsForce(r.status));

const skills = (n: number) => `${n} skill${n === 1 ? '' : 's'}`;

export interface InstallSkillsDialogProps {
  /** The projects a skill can be installed into (their `.rupu/skills`). */
  projects: readonly ProjectRow[];
  onClose: () => void;
  /** After an install landed: the owner re-reads its list. */
  onInstalled: () => void;
}

export function InstallSkillsDialog({ projects, onClose, onInstalled }: InstallSkillsDialogProps) {
  const [source, setSource] = useState('');
  const [ref, setRef] = useState('');
  const [project, setProject] = useState('');
  const [force, setForce] = useState(false);
  const [preview, setPreview] = useState<SkillInstallPreview | null>(null);
  const [picked, setPicked] = useState<ReadonlySet<string>>(new Set());
  const [busy, setBusy] = useState<'finding' | 'installing' | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [result, setResult] = useState<SkillInstallReport | null>(null);
  const sourceHelp = useId();
  // The install would replace installed copies: asked before it is sent.
  const [confirming, setConfirming] = useState(false);

  const request = { source, ref, project: project || undefined };

  // Any change to what would be installed, or where, makes the preview stale.
  const edit = (set: (v: string) => void) => (v: string) => {
    set(v);
    setPreview(null);
    setPicked(new Set());
    setConfirming(false);
    setResult(null);
    setError(null);
  };

  async function find(e?: FormEvent) {
    e?.preventDefault();
    if (!source.trim() || busy) return;
    setBusy('finding');
    setError(null);
    setResult(null);
    try {
      const p = await previewSkillInstall(request);
      setPreview(p);
      setPicked(new Set(p.found.filter((r) => r.status === 'new').map((r) => r.name)));
    } catch (err: unknown) {
      setPreview(null);
      setError(apiErrorMessage(err));
    } finally {
      setBusy(null);
    }
  }

  async function install() {
    if (!preview || picked.size === 0 || busy) return;
    setConfirming(false);
    setBusy('installing');
    setError(null);
    try {
      const names = preview.found.filter((r) => picked.has(r.name)).map((r) => r.name);
      const report = await installSkills({ ...request, skills: names, force });
      setResult(report);
      onInstalled();
      // What the layer holds now, from the server.
      const p = await previewSkillInstall(request);
      setPreview(p);
      setPicked(new Set());
    } catch (err: unknown) {
      setError(apiErrorMessage(err));
    } finally {
      setBusy(null);
    }
  }

  function toggleForce(on: boolean) {
    setForce(on);
    setConfirming(false);
    if (!on && preview) {
      const replace = new Set(preview.found.filter((r) => needsForce(r.status)).map((r) => r.name));
      setPicked((prev) => new Set([...prev].filter((n) => !replace.has(n))));
    }
  }

  function toggle(name: string) {
    setConfirming(false);
    setPicked((prev) => {
      const next = new Set(prev);
      if (next.has(name)) next.delete(name);
      else next.add(name);
      return next;
    });
  }

  const target = project ? projects.find((p) => p.ws_id === project) : null;
  // The ticked skills whose installed copy the install would replace.
  const replacing = (preview?.found ?? []).filter((r) => picked.has(r.name) && needsForce(r.status)).map((r) => r.name);

  return (
    <DialogFrame title="Install skills" onRequestClose={onClose} className="max-w-xl">
      <form onSubmit={find} className="mt-4 space-y-3">
        <div>
          <label className="block">
            <span className="text-ui font-medium text-ink">Source</span>
            <Input
              className="mt-1 max-w-none font-mono"
              value={source}
              onChange={(e) => edit(setSource)(e.target.value)}
              placeholder="owner/repo"
              spellCheck={false}
              autoComplete="off"
              aria-describedby={sourceHelp}
            />
          </label>
          <p id={sourceHelp} className="mt-1 text-note text-ink-mute">
            A local directory (absolute), owner/repo, or an https git URL. Pick a branch, tag or commit under Ref.
          </p>
        </div>
        <div className="grid grid-cols-2 gap-3">
          <label className="block">
            <span className="text-ui font-medium text-ink">Ref</span>
            <Input
              className="mt-1 max-w-none font-mono"
              value={ref}
              onChange={(e) => edit(setRef)(e.target.value)}
              placeholder="main"
              spellCheck={false}
              autoComplete="off"
            />
          </label>
          <label className="block">
            <span className="text-ui font-medium text-ink">Install to</span>
            <Select className="mt-1 w-full" value={project} onChange={(e) => edit(setProject)(e.target.value)}>
              <option value="">Global (~/.rupu/skills)</option>
              {projects.map((p) => (
                <option key={p.ws_id} value={p.ws_id}>
                  {p.name} (.rupu/skills)
                </option>
              ))}
            </Select>
          </label>
        </div>
      </form>

      {preview && (
        <div className="mt-4 overflow-hidden rounded-lg border border-border">
          <div className="flex items-center gap-3 border-b border-border bg-surface px-3 py-1.5 text-note font-medium text-ink-mute">
            <span className="w-4" aria-hidden />
            <span className="w-28 shrink-0">Found in source</span>
            <span className="min-w-0 flex-1">Description</span>
            <span>Status</span>
          </div>
          <ul aria-label="Found in source" className="max-h-72 divide-y divide-border overflow-y-auto">
            {preview.found.map((r) => {
              const can = installable(r, force);
              const s = STATUS[r.status];
              const why =
                r.status === 'up_to_date'
                  ? 'Already installed from this source'
                  : needsForce(r.status) && !force
                    ? 'Tick "Replace locally edited copies" to replace the installed copy'
                    : undefined;
              return (
                <li key={`${r.rel}/${r.name}`} title={why}>
                  <label className={cn('flex items-center gap-3 px-3 py-2', can ? 'cursor-pointer hover:bg-surface-hover' : 'opacity-80')}>
                    <input
                      type="checkbox"
                      aria-label={r.name}
                      className="h-4 w-4 accent-brand-600"
                      checked={picked.has(r.name)}
                      disabled={!can || busy !== null}
                      onChange={() => toggle(r.name)}
                    />
                    <span className="w-28 shrink-0 truncate font-mono text-ui text-ink">{r.name}</span>
                    <span className="min-w-0 flex-1 truncate text-note text-ink-dim">
                      {r.reason ?? r.description ?? '-'}
                    </span>
                    <StatusChip tone={s.tone}>{s.label}</StatusChip>
                  </label>
                </li>
              );
            })}
          </ul>
        </div>
      )}

      {preview && (
        <label className="mt-3 flex items-center gap-2 text-ui text-err">
          <input
            type="checkbox"
            className="h-4 w-4 accent-err"
            checked={force}
            disabled={busy !== null}
            onChange={(e) => toggleForce(e.target.checked)}
          />
          Replace locally edited copies
        </label>
      )}

      {result && (
        <div role="status" className="mt-3 rounded-lg bg-ok-bg px-3 py-2 text-note text-ok ring-1 ring-ok/30">
          {result.installed.length > 0 && <p>Installed {result.installed.map((i) => i.name).join(', ')}.</p>}
          {result.skipped.map((s) => (
            <p key={s.name} className="text-warn">
              Skipped {s.name}: {s.reason}
            </p>
          ))}
          {result.installed.some((i) => !i.tracked) && (
            <p className="text-ink-dim">
              Installed from a directory outside {target?.name ?? 'the project'}: Update can't follow it.
            </p>
          )}
        </div>
      )}

      {confirming && (
        <p className="mt-3 rounded-lg border border-err/30 bg-err-bg px-3 py-2 text-note text-err">
          {replacing.length === 1
            ? `Replace the installed copy of ${replacing[0]}? Its local edits are lost.`
            : `Replace the installed copies of ${replacing.join(', ')}? Their local edits are lost.`}
        </p>
      )}

      {error && (
        <p role="alert" className="mt-3 rounded-lg border border-err/30 bg-err-bg px-3 py-2 text-note text-err">
          {error}
        </p>
      )}

      <div className="mt-5 flex justify-end gap-2">
        {confirming ? (
          <>
            <Button variant="secondary" onClick={() => setConfirming(false)}>
              Back
            </Button>
            <Button variant="danger" onClick={install}>
              Replace and install
            </Button>
          </>
        ) : (
          <>
            <Button variant="secondary" onClick={onClose}>
              {result ? 'Close' : 'Cancel'}
            </Button>
            {preview ? (
              <Button
                variant={replacing.length > 0 ? 'danger' : 'solid'}
                disabled={picked.size === 0 || busy !== null}
                onClick={() => (replacing.length > 0 ? setConfirming(true) : install())}
              >
                {busy === 'installing' ? 'Installing…' : `Install ${skills(picked.size)}`}
              </Button>
            ) : (
              <Button variant="solid" disabled={!source.trim() || busy !== null} onClick={() => find()}>
                {busy === 'finding' ? 'Finding skills…' : 'Find skills'}
              </Button>
            )}
          </>
        )}
      </div>
    </DialogFrame>
  );
}

export default InstallSkillsDialog;
