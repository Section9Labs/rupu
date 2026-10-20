// Project → Config → Repositories (operator MCP Plan 3b, UI-5): the tracked
// checkouts (`GET /api/repos/tracked`, the `~/.rupu/repos/` registry) that
// sit inside this project's directory — what autoflows and repo run targets
// reuse instead of cloning. Prefer makes one the repo's preferred checkout
// (`POST /api/repos/tracked/prefer`), Forget drops that one path after a
// confirm (`DELETE /api/repos/tracked/:id?path=`), and Attach tracks a
// checkout (`POST /api/repos/tracked`). Every change shows once the next read
// reflects it.

import { useCallback, useEffect, useId, useState, type FormEvent } from 'react';
import { apiErrorMessage } from '../../lib/api';
import { admin, type TrackedRepo } from '../../lib/api3b/admin';
import { relativeTime } from '../../lib/time';
import { Button } from '../ui/Button';
import { Input } from '../ui/Input';
import { Skeleton } from '../ui/Skeleton';
import { StatusChip } from '../ui/StatusChip';
import { DialogFrame } from '../customers/DialogFrame';

/** One checkout of a tracked repo. */
interface Checkout {
  repo: TrackedRepo;
  path: string;
}

const inside = (path: string, root: string) => {
  const r = root.replace(/\/+$/, '');
  return path === r || path.startsWith(`${r}/`);
};

/** A checkout's path as the project sees it: `project root`, or relative to it. */
export function relativeTo(path: string, root: string): string {
  const r = root.replace(/\/+$/, '');
  return path === r ? 'project root' : path.slice(r.length + 1);
}

/** The checkouts inside `root`, one per (repo, path). */
export function checkoutsIn(repos: TrackedRepo[], root: string): Checkout[] {
  return repos.flatMap((repo) => {
    const paths = [...new Set([repo.preferred_path, ...repo.known_paths])];
    return paths.filter((p) => inside(p, root)).map((path) => ({ repo, path }));
  });
}

function AttachDialog({ projectPath, onClose, onAttached }: { projectPath: string; onClose: () => void; onAttached: () => void }) {
  const repoId = useId();
  const pathId = useId();
  const [repo, setRepo] = useState('');
  const [path, setPath] = useState(projectPath);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function submit(e: FormEvent) {
    e.preventDefault();
    setBusy(true);
    setError(null);
    try {
      await admin.attachRepo({ repo: repo.trim(), path: path.trim() });
      onAttached();
    } catch (err) {
      setError(apiErrorMessage(err));
    } finally {
      setBusy(false);
    }
  }

  return (
    <DialogFrame title="Attach a checkout" onRequestClose={onClose} className="max-w-lg">
      <form onSubmit={(e) => void submit(e)} className="mt-3 space-y-3">
        <div>
          <label htmlFor={repoId} className="mb-1 block text-note font-semibold text-ink">
            Repository
          </label>
          <Input
            id={repoId}
            value={repo}
            onChange={(e) => setRepo(e.target.value)}
            placeholder="github:owner/repo"
            className="max-w-none font-mono"
            spellCheck={false}
          />
        </div>
        <div>
          <label htmlFor={pathId} className="mb-1 block text-note font-semibold text-ink">
            Path
          </label>
          <Input
            id={pathId}
            value={path}
            onChange={(e) => setPath(e.target.value)}
            className="max-w-none font-mono"
            spellCheck={false}
          />
          <p className="mt-1 text-note text-ink-mute">
            An absolute path to a git checkout whose <span className="font-mono">origin</span> is this repository.
          </p>
        </div>
        {error && (
          <p role="alert" className="text-note text-err">
            {error}
          </p>
        )}
        <div className="flex justify-end gap-2 pt-1">
          <Button variant="secondary" onClick={onClose}>
            Cancel
          </Button>
          <Button type="submit" variant="solid" disabled={busy || !repo.trim() || !path.trim()}>
            {busy ? 'Attaching…' : 'Attach'}
          </Button>
        </div>
      </form>
    </DialogFrame>
  );
}

export function ProjectRepositories({ projectPath }: { projectPath: string }) {
  const [repos, setRepos] = useState<TrackedRepo[] | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [confirming, setConfirming] = useState<Checkout | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [attaching, setAttaching] = useState(false);

  const load = useCallback(async () => {
    try {
      setRepos(await admin.getTrackedRepos());
      setLoadError(null);
    } catch (e) {
      setLoadError(apiErrorMessage(e));
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  async function act(key: string, fn: () => Promise<unknown>) {
    setBusy(key);
    setActionError(null);
    try {
      await fn();
    } catch (e) {
      setActionError(apiErrorMessage(e));
    } finally {
      await load();
      setBusy(null);
    }
  }

  const checkouts = repos ? checkoutsIn(repos, projectPath) : [];
  const keyOf = (c: Checkout) => `${c.repo.id}:${c.path}`;

  return (
    <section className="mt-6 rounded-xl border border-border bg-panel px-5 py-4 shadow-card">
      <div className="flex items-start gap-3">
        <div className="min-w-0 flex-1">
          <h2 className="text-sm font-semibold text-ink">Repositories</h2>
          <p className="mt-0.5 text-note text-ink-mute">
            Tracked checkouts in this project, which autoflows and repo run targets reuse instead of cloning.
          </p>
        </div>
        <Button variant="secondary" size="sm" onClick={() => setAttaching(true)}>
          Attach
        </Button>
      </div>

      {loadError && (
        <p role="alert" className="mt-3 text-note text-err">
          {loadError}
        </p>
      )}
      {actionError && (
        <p role="alert" className="mt-3 text-note text-err">
          {actionError}
        </p>
      )}

      {repos === null && !loadError ? (
        <div role="status" aria-label="Loading repositories…" className="mt-3 space-y-2">
          <Skeleton className="h-4 w-1/2" />
          <Skeleton className="h-4 w-2/5" />
        </div>
      ) : checkouts.length === 0 && !loadError ? (
        <p className="mt-3 text-sm text-ink-dim">No tracked checkout is in this project.</p>
      ) : checkouts.length > 0 ? (
        <table className="mt-3 w-full text-note">
          <thead className="bg-surface">
            <tr>
              {['Repository', 'Checkout', 'Default branch', 'Last seen', ''].map((h, i) => (
                <th
                  key={i}
                  className="whitespace-nowrap px-3 py-2 text-left text-meta font-semibold uppercase tracking-wide text-ink-mute"
                >
                  {h}
                </th>
              ))}
            </tr>
          </thead>
          <tbody className="divide-y divide-border">
            {checkouts.map((c) => {
              const preferred = c.path === c.repo.preferred_path;
              const key = keyOf(c);
              return (
                <tr key={key} aria-label={`${c.repo.repo} ${c.path}`}>
                  <td className="px-3 py-2 font-mono text-ink">{c.repo.repo}</td>
                  <td className="px-3 py-2">
                    <span className="flex flex-wrap items-center gap-2">
                      <span className="font-mono text-ink-dim" title={c.path}>
                        {relativeTo(c.path, projectPath)}
                      </span>
                      {preferred && <StatusChip tone="completed">preferred</StatusChip>}
                    </span>
                  </td>
                  <td className="px-3 py-2 font-mono text-ink-dim">{c.repo.default_branch ?? '—'}</td>
                  <td className="whitespace-nowrap px-3 py-2 text-ink-mute" title={c.repo.last_seen_at}>
                    {relativeTime(c.repo.last_seen_at)}
                  </td>
                  <td className="px-3 py-2">
                    <span className="flex items-center justify-end gap-1">
                      {!preferred && (
                        <Button
                          variant="ring"
                          disabled={busy !== null}
                          onClick={() => void act(key, () => admin.preferRepo({ repo: c.repo.repo, path: c.path }))}
                          aria-label={`Prefer ${c.path}`}
                        >
                          {busy === key ? 'Preferring…' : 'Prefer'}
                        </Button>
                      )}
                      <Button
                        variant="ring-danger"
                        disabled={busy !== null}
                        onClick={() => setConfirming(c)}
                        aria-label={`Forget ${c.path}`}
                      >
                        {busy === key ? 'Forgetting…' : 'Forget'}
                      </Button>
                    </span>
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      ) : null}

      {confirming && (
        <div
          role="group"
          aria-label="Confirm forget"
          className="mt-3 flex flex-wrap items-center gap-2 rounded-lg border border-err/30 bg-err-bg px-3 py-2 text-note"
        >
          <span className="text-ink">
            Forget{' '}
            <span className="font-mono" title={confirming.path}>
              {relativeTo(confirming.path, projectPath) === 'project root'
                ? 'the project root'
                : relativeTo(confirming.path, projectPath)}
            </span>{' '}
            as a checkout of{' '}
            <span className="font-mono">{confirming.repo.repo}</span>? The files stay; rupu stops reusing it.
          </span>
          <span className="flex-1" />
          <Button variant="secondary" size="sm" onClick={() => setConfirming(null)}>
            Cancel
          </Button>
          <Button
            variant="danger"
            size="sm"
            onClick={() => {
              const c = confirming;
              setConfirming(null);
              void act(keyOf(c), () => admin.forgetRepo(c.repo.id, c.path));
            }}
          >
            Forget
          </Button>
        </div>
      )}

      {attaching && (
        <AttachDialog
          projectPath={projectPath}
          onClose={() => setAttaching(false)}
          onAttached={() => {
            setAttaching(false);
            void load();
          }}
        />
      )}
    </section>
  );
}

export default ProjectRepositories;
