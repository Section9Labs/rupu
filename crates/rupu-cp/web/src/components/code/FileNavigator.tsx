import { useEffect, useMemo, useRef, useState } from 'react';
import { Loader2, Search } from 'lucide-react';
import { api, type FindingRecord, type FileListResult } from '../../lib/api';
import { SEVERITY_STYLE, type Severity } from '../../lib/severity';
import FileTree, { fileFindingSummary } from './FileTree';

export interface FileNavigatorProps {
  wsId: string;
  findings: FindingRecord[];
  selectedPath: string | null;
  onSelect: (path: string) => void;
}

type Mode = 'all' | 'findings';

/** Split a workspace-relative path into its directory (may be `''`) and
 *  basename, for the "basename bold + dir dim" row rendering shared by the
 *  Findings list and the project-wide search list. */
function splitPath(path: string): { dir: string; base: string } {
  const i = path.lastIndexOf('/');
  return i === -1 ? { dir: '', base: path } : { dir: path.slice(0, i), base: path.slice(i + 1) };
}

function ToggleButton({
  active,
  onClick,
  children,
  testId,
}: {
  active: boolean;
  onClick: () => void;
  children: React.ReactNode;
  testId: string;
}) {
  return (
    <button
      type="button"
      data-testid={testId}
      aria-pressed={active}
      onClick={onClick}
      className={`rounded px-2 py-0.5 text-[11px] font-medium ${
        active ? 'bg-panel text-ink ring-1 ring-border' : 'text-ink-dim hover:bg-surface-hover'
      }`}
    >
      {children}
    </button>
  );
}

function FlatFindingBadge({ findings, path }: { findings: FindingRecord[]; path: string }) {
  const r = fileFindingSummary(findings, path);
  if (!r) return null;
  const style = SEVERITY_STYLE[r.worst as Severity] ?? SEVERITY_STYLE.info;
  return (
    <span
      data-testid={`nav-badge-${path}`}
      className={`ml-auto shrink-0 rounded-full px-1.5 text-[10px] ${style.pill}`}
    >
      {r.count}
    </span>
  );
}

/** One row in a flat (non-tree) file list: basename bold + dir dim, plus a
 *  finding badge when the file has findings. Shared by Findings mode and
 *  the project-wide search list in All mode. */
function FlatFileRow({
  path,
  findings,
  active,
  onSelect,
}: {
  path: string;
  findings: FindingRecord[];
  active: boolean;
  onSelect: (p: string) => void;
}) {
  const { dir, base } = splitPath(path);
  return (
    <button
      type="button"
      data-testid={`nav-row-${path}`}
      title={path}
      onClick={() => onSelect(path)}
      className={`flex w-full items-center gap-1 rounded px-2 py-0.5 text-left text-[12px] ${
        active ? 'bg-panel text-ink ring-1 ring-border' : 'text-ink-dim hover:bg-surface-hover'
      }`}
    >
      <span className="truncate">
        {dir && <span className="text-ink-mute">{dir}/</span>}
        <span className="font-semibold text-ink">{base}</span>
      </span>
      <FlatFindingBadge findings={findings} path={path} />
    </button>
  );
}

function EmptyRow({ children }: { children: React.ReactNode }) {
  return <div className="px-2 py-2 text-[12px] text-ink-dim">{children}</div>;
}

export default function FileNavigator({ wsId, findings, selectedPath, onSelect }: FileNavigatorProps) {
  const [mode, setMode] = useState<Mode>('all');
  const [search, setSearch] = useState('');
  const [allFiles, setAllFiles] = useState<FileListResult | null>(null);
  const [allFilesLoading, setAllFilesLoading] = useState(false);
  const [allFilesErr, setAllFilesErr] = useState<string | null>(null);

  const query = search.trim();
  const needsAllFiles = mode === 'all' && query !== '';

  // Guards the lazy fetch below with a ref (not state): `allFilesLoading` is
  // *set inside* the effect, so if it were also a dependency, flipping it to
  // `true` would re-run the effect — and React runs the previous effect's
  // cleanup first, flipping `live` to `false` before the fetch even
  // resolves, silently dropping the result. A ref sidesteps that without
  // adding a self-referential dependency.
  const requestedFor = useRef<string | null>(null);

  // Reset the cache when the project changes, so a different workspace's
  // search doesn't see a stale file list.
  useEffect(() => {
    requestedFor.current = null;
    setAllFiles(null);
    setAllFilesErr(null);
  }, [wsId]);

  // Lazy fetch: only hit the flat project-files endpoint once the user
  // actually needs it (All mode + a non-empty search), and only once per
  // workspace — cached in state for the rest of the session.
  useEffect(() => {
    if (!needsAllFiles || requestedFor.current === wsId) return;
    requestedFor.current = wsId;
    let live = true;
    setAllFilesLoading(true);
    api
      .getProjectFiles(wsId)
      .then((r) => {
        if (live) setAllFiles(r);
      })
      .catch((e) => {
        if (live) setAllFilesErr(String(e?.message ?? e));
      })
      .finally(() => {
        if (live) setAllFilesLoading(false);
      });
    return () => {
      live = false;
    };
  }, [needsAllFiles, wsId]);

  // Distinct, sorted file paths that have at least one finding.
  const findingFiles = useMemo(() => {
    const set = new Set<string>();
    for (const f of findings) {
      if (f.file_path) set.add(f.file_path);
    }
    return Array.from(set).sort();
  }, [findings]);

  const filteredFindingFiles = useMemo(() => {
    if (!query) return findingFiles;
    const q = query.toLowerCase();
    return findingFiles.filter((p) => p.toLowerCase().includes(q));
  }, [findingFiles, query]);

  const filteredAllFiles = useMemo(() => {
    if (!allFiles) return [];
    const q = query.toLowerCase();
    return allFiles.files.filter((p) => p.toLowerCase().includes(q));
  }, [allFiles, query]);

  return (
    <div>
      <div className="sticky top-0 z-10 flex flex-col gap-1.5 border-b border-border bg-surface p-1.5">
        <div className="relative">
          <Search size={12} className="pointer-events-none absolute left-2 top-1.5 text-ink-mute" />
          <input
            type="text"
            value={search}
            onChange={(e) => setSearch(e.target.value)}
            placeholder="Filter files…"
            aria-label="Filter files"
            className="w-full rounded border border-border bg-surface py-1 pl-6 pr-2 text-[12px] text-ink placeholder:text-ink-mute focus:outline-none focus:ring-1 focus:ring-border"
          />
        </div>
        <div className="flex gap-1">
          <ToggleButton testId="nav-mode-all" active={mode === 'all'} onClick={() => setMode('all')}>
            All
          </ToggleButton>
          <ToggleButton
            testId="nav-mode-findings"
            active={mode === 'findings'}
            onClick={() => setMode('findings')}
          >
            Findings
          </ToggleButton>
        </div>
      </div>

      <div className="py-1">
        {mode === 'findings' ? (
          findingFiles.length === 0 ? (
            <EmptyRow>No findings in this project.</EmptyRow>
          ) : filteredFindingFiles.length === 0 ? (
            <EmptyRow>No files match '{query}'.</EmptyRow>
          ) : (
            filteredFindingFiles.map((p) => (
              <FlatFileRow
                key={p}
                path={p}
                findings={findings}
                active={selectedPath === p}
                onSelect={onSelect}
              />
            ))
          )
        ) : query === '' ? (
          <FileTree wsId={wsId} findings={findings} selectedPath={selectedPath} onSelect={onSelect} />
        ) : allFilesErr ? (
          <EmptyRow>Search error: {allFilesErr}</EmptyRow>
        ) : allFilesLoading && !allFiles ? (
          <div className="flex items-center gap-2 px-2 py-2 text-[12px] text-ink-dim">
            <Loader2 size={12} className="animate-spin" /> Searching…
          </div>
        ) : filteredAllFiles.length === 0 ? (
          <EmptyRow>No files match '{query}'.</EmptyRow>
        ) : (
          <>
            <div className="px-2 pb-1 text-[11px] text-ink-mute">
              {filteredAllFiles.length} result{filteredAllFiles.length === 1 ? '' : 's'}
              {allFiles?.truncated ? ' (truncated — refine search)' : ''}
            </div>
            {filteredAllFiles.map((p) => (
              <FlatFileRow
                key={p}
                path={p}
                findings={findings}
                active={selectedPath === p}
                onSelect={onSelect}
              />
            ))}
          </>
        )}
      </div>
    </div>
  );
}
