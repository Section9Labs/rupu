// Directory picker: free-text path with fuzzy-complete over past projects, plus
// a browse list (drill into subdirs / go up). The chosen absolute path is the
// value (sent as the run's working_dir).
import { useEffect, useState } from 'react';
import { api, type FsEntry, type ProjectRow } from '../lib/api';

export function matchProjects(paths: string[], query: string): string[] {
  const q = query.trim().toLowerCase();
  if (!q) return paths;
  return paths.filter((p) => p.toLowerCase().includes(q));
}

export default function DirectoryPicker({
  value,
  onChange,
}: {
  value: string;
  onChange: (v: string) => void;
}) {
  const [projects, setProjects] = useState<string[]>([]);
  const [dirs, setDirs] = useState<FsEntry[]>([]);
  const [parent, setParent] = useState<string | null>(null);
  const [browsePath, setBrowsePath] = useState<string>('');
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    api
      .getProjects()
      .then((ps: ProjectRow[]) => setProjects(ps.map((p) => p.path)))
      .catch(() => setProjects([]));
  }, []);

  function load(path?: string) {
    api
      .browseDir(path)
      .then((r) => {
        setDirs(r.dirs);
        setParent(r.parent);
        setBrowsePath(r.path);
        setError(null);
      })
      .catch((e: unknown) => setError(e instanceof Error ? e.message : 'Cannot read directory'));
  }

  // Initial browse (home) when the picker mounts.
  useEffect(() => {
    load(value || undefined);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const fieldCls =
    'w-full rounded-md border border-border bg-white px-2.5 py-1.5 text-[13px] text-ink placeholder:text-ink-mute focus:border-brand-500 focus:outline-none';
  const projMatches = matchProjects(projects, value).slice(0, 6);

  return (
    <div className="space-y-2">
      <input
        type="text"
        value={value}
        onChange={(e) => onChange(e.target.value)}
        placeholder="/path/to/project"
        aria-label="Directory path"
        className={fieldCls}
      />

      {projMatches.length > 0 && (
        <div className="flex flex-wrap gap-1">
          {projMatches.map((p) => (
            <button
              key={p}
              type="button"
              onClick={() => {
                onChange(p);
                load(p);
              }}
              className="rounded border border-border bg-slate-50 px-1.5 py-0.5 text-[11px] font-mono text-ink-dim hover:bg-slate-100"
            >
              {p}
            </button>
          ))}
        </div>
      )}

      <div className="rounded-md border border-border bg-white">
        <div className="flex items-center justify-between border-b border-border px-2 py-1 text-[11px] text-ink-mute">
          <span className="truncate font-mono">{browsePath || '…'}</span>
          <button
            type="button"
            onClick={() => onChange(browsePath)}
            className="ml-2 shrink-0 font-medium text-brand-600 hover:text-brand-700"
          >
            use this
          </button>
        </div>
        <ul className="max-h-44 overflow-auto py-1">
          {parent && (
            <li>
              <button
                type="button"
                onClick={() => load(parent)}
                className="block w-full px-2 py-1 text-left text-[12px] font-mono text-ink-dim hover:bg-slate-50"
              >
                ../
              </button>
            </li>
          )}
          {dirs.map((d) => (
            <li key={d.path}>
              <button
                type="button"
                onClick={() => {
                  onChange(d.path);
                  load(d.path);
                }}
                className="block w-full px-2 py-1 text-left text-[12px] font-mono text-ink hover:bg-slate-50"
              >
                {d.name}/
              </button>
            </li>
          ))}
          {dirs.length === 0 && !parent && (
            <li className="px-2 py-1 text-[12px] text-ink-mute">no subdirectories</li>
          )}
        </ul>
      </div>
      {error && <p className="text-[12px] text-red-700">{error}</p>}
    </div>
  );
}
