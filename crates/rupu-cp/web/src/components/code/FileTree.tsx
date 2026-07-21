import { useEffect, useMemo, useState } from 'react';
import { ChevronRight, ChevronDown, File as FileIcon, Loader2 } from 'lucide-react';
import { api, type FindingRecord, type TreeEntry, type TreeResult } from '../../lib/api';
import { SEVERITY_STYLE, severityRank, type Severity } from '../../lib/severity';

export interface FileTreeProps {
  wsId: string;
  findings: FindingRecord[];
  selectedPath: string | null;
  onSelect: (path: string) => void;
}

/** Worst severity among findings whose file_path is at-or-under `prefix`
 *  (folders) or exactly `path` (files), plus a count. Null when none. */
function rollup(findings: FindingRecord[], prefix: string, isDir: boolean) {
  const match = findings.filter((f) => {
    if (!f.file_path) return false;
    return isDir ? f.file_path === prefix || f.file_path.startsWith(prefix + '/') : f.file_path === prefix;
  });
  if (match.length === 0) return null;
  const worst = match.reduce<Severity>((acc, f) => {
    const s = (f.severity as Severity) ?? 'info';
    return severityRank(s) > severityRank(acc) ? s : acc;
  }, 'info');
  return { worst, count: match.length };
}

function Badge({ node, findings }: { node: TreeEntry; findings: FindingRecord[] }) {
  const r = rollup(findings, node.path, node.kind === 'dir');
  if (!r) return null;
  const style = SEVERITY_STYLE[r.worst] ?? SEVERITY_STYLE.info;
  return (
    <span
      data-testid={`badge-${node.path}`}
      className={`ml-auto shrink-0 rounded-full px-1.5 text-[10px] ${style.pill}`}
    >
      {r.count}
    </span>
  );
}

function Dir({
  node,
  depth,
  wsId,
  findings,
  selectedPath,
  onSelect,
}: {
  node: TreeEntry;
  depth: number;
  wsId: string;
  findings: FindingRecord[];
  selectedPath: string | null;
  onSelect: (p: string) => void;
}) {
  const [open, setOpen] = useState(false);
  const [children, setChildren] = useState<TreeEntry[] | null>(null);
  const [loading, setLoading] = useState(false);

  const toggle = () => {
    const next = !open;
    setOpen(next);
    if (next && children === null) {
      setLoading(true);
      api
        .getProjectTree(wsId, node.path)
        .then((r) => setChildren(r.entries))
        .finally(() => setLoading(false));
    }
  };

  return (
    <div>
      <button
        type="button"
        onClick={toggle}
        className="flex w-full items-center gap-1 rounded px-1 py-0.5 text-left text-[12px] text-ink-dim hover:bg-surface-hover"
        style={{ paddingLeft: `${depth * 12 + 4}px` }}
      >
        {open ? <ChevronDown size={13} /> : <ChevronRight size={13} />}
        <span className="text-ink">{node.name}</span>
        <Badge node={node} findings={findings} />
      </button>
      {open &&
        (loading ? (
          <div className="pl-6 py-0.5 text-ink-mute">
            <Loader2 size={12} className="animate-spin" />
          </div>
        ) : (
          children?.map((c) =>
            c.kind === 'dir' ? (
              <Dir
                key={c.path}
                node={c}
                depth={depth + 1}
                wsId={wsId}
                findings={findings}
                selectedPath={selectedPath}
                onSelect={onSelect}
              />
            ) : (
              <FileNode
                key={c.path}
                node={c}
                depth={depth + 1}
                findings={findings}
                selectedPath={selectedPath}
                onSelect={onSelect}
              />
            ),
          )
        ))}
    </div>
  );
}

function FileNode({
  node,
  depth,
  findings,
  selectedPath,
  onSelect,
}: {
  node: TreeEntry;
  depth: number;
  findings: FindingRecord[];
  selectedPath: string | null;
  onSelect: (p: string) => void;
}) {
  const active = selectedPath === node.path;
  return (
    <button
      type="button"
      onClick={() => onSelect(node.path)}
      className={`flex w-full items-center gap-1 rounded px-1 py-0.5 text-left text-[12px] ${active ? 'bg-panel text-ink ring-1 ring-border' : 'text-ink-dim hover:bg-surface-hover'}`}
      style={{ paddingLeft: `${depth * 12 + 18}px` }}
    >
      <FileIcon size={12} className="shrink-0 text-ink-mute" />
      <span className="truncate">{node.name}</span>
      <Badge node={node} findings={findings} />
    </button>
  );
}

export default function FileTree({ wsId, findings, selectedPath, onSelect }: FileTreeProps) {
  const [root, setRoot] = useState<TreeResult | null>(null);
  const [err, setErr] = useState<string | null>(null);

  useEffect(() => {
    let live = true;
    api
      .getProjectTree(wsId)
      .then((r) => live && setRoot(r))
      .catch((e) => live && setErr(String(e?.message ?? e)));
    return () => {
      live = false;
    };
  }, [wsId]);

  const entries = useMemo(() => root?.entries ?? [], [root]);
  if (err) return <div className="p-2 text-[12px] text-err">Tree error: {err}</div>;
  if (!root)
    return (
      <div className="flex items-center gap-2 p-2 text-[12px] text-ink-dim">
        <Loader2 size={12} className="animate-spin" /> Loading files…
      </div>
    );

  return (
    <div className="py-1">
      {entries.map((e) =>
        e.kind === 'dir' ? (
          <Dir key={e.path} node={e} depth={0} wsId={wsId} findings={findings} selectedPath={selectedPath} onSelect={onSelect} />
        ) : (
          <FileNode key={e.path} node={e} depth={0} findings={findings} selectedPath={selectedPath} onSelect={onSelect} />
        ),
      )}
    </div>
  );
}
