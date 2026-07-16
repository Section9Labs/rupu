/**
 * AstTree — recursive CST viewer for the transcript drill-down's
 * "view AST" affordance (a sibling of `SourcePreview`'s "view source").
 *
 * Fetches lazily via `api.readAst` on mount (and whenever `runId` / `path`
 * / `line` / `col` / `host` change) — callers control lazy mounting by not
 * rendering this component until the viewer is opened.
 *
 * States:
 *   - loading    → "Loading AST…"
 *   - error      → fetch/network failure message
 *   - unavailable (`available: false`) → the backend's `reason` text
 *   - loaded     → recursive, collapsible tree. Named-only by default (an
 *                  "show anonymous" checkbox reveals unnamed/anonymous
 *                  nodes); the `matched` node is highlighted (amber bg) and
 *                  its ancestor chain is auto-expanded so it's visible as
 *                  soon as the tree renders; a `truncated` response adds a
 *                  small note.
 */

import * as React from 'react';
import { ChevronRight } from 'lucide-react';
import { api } from '../../lib/api';
import type { AstNode, AstResponse } from '../../lib/api';
import { cn } from '../../lib/cn';

type AstTreeState =
  | { status: 'loading' }
  | { status: 'error'; message: string }
  | { status: 'loaded'; response: AstResponse };

export interface AstTreeProps {
  runId: string;
  path: string;
  line: number;
  col: number;
  host?: string;
}

/**
 * Depth-first search for the `matched` node, returning the path keys of
 * every node from the root down to (but not including) the matched node
 * itself — i.e. the set of nodes that must be expanded for the matched node
 * to be visible. Returns `null` when no descendant is matched.
 */
function findMatchedAncestorPaths(node: AstNode, path: string): string[] | null {
  if (node.matched) return [];
  for (let i = 0; i < node.children.length; i++) {
    const found = findMatchedAncestorPaths(node.children[i], `${path}.${i}`);
    if (found) return [path, ...found];
  }
  return null;
}

function TreeNode({
  node,
  path,
  depth,
  expanded,
  onToggle,
  showAnonymous,
}: {
  node: AstNode;
  path: string;
  depth: number;
  expanded: ReadonlySet<string>;
  onToggle: (path: string) => void;
  showAnonymous: boolean;
}) {
  const hasChildren = node.children.length > 0;
  const isExpanded = expanded.has(path);
  const range = `${node.startLine}:${node.startCol}-${node.endLine}:${node.endCol}`;

  const row = (
    <span className="flex min-w-0 items-center gap-1.5">
      {node.field && <span className="text-ink-mute">{node.field}:</span>}
      <span className={cn('font-mono', !node.named && 'text-ink-mute opacity-70')}>{node.kind}</span>
      <span className="text-ink-mute text-[10px]">{range}</span>
    </span>
  );

  return (
    <div>
      <div
        data-matched={node.matched ? 'true' : undefined}
        className={cn('flex items-center gap-1 rounded px-1 py-0.5', node.matched && 'bg-warn-bg')}
        style={{ paddingLeft: depth * 14 }}
      >
        {hasChildren ? (
          <button
            type="button"
            onClick={() => onToggle(path)}
            className="flex shrink-0 items-center"
            aria-label={isExpanded ? `Collapse ${node.kind}` : `Expand ${node.kind}`}
          >
            <ChevronRight
              size={12}
              className={cn('text-ink-mute transition-transform', isExpanded && 'rotate-90')}
            />
            {row}
          </button>
        ) : (
          <span className="flex items-center gap-1 pl-[15px]">{row}</span>
        )}
      </div>
      {hasChildren && isExpanded && (
        <div>
          {node.children.map((child, i) => {
            if (!showAnonymous && !child.named) return null;
            return (
              <TreeNode
                key={i}
                node={child}
                path={`${path}.${i}`}
                depth={depth + 1}
                expanded={expanded}
                onToggle={onToggle}
                showAnonymous={showAnonymous}
              />
            );
          })}
        </div>
      )}
    </div>
  );
}

export default function AstTree({ runId, path, line, col, host }: AstTreeProps) {
  const [state, setState] = React.useState<AstTreeState>({ status: 'loading' });
  const [expanded, setExpanded] = React.useState<ReadonlySet<string>>(new Set());
  const [showAnonymous, setShowAnonymous] = React.useState(false);

  React.useEffect(() => {
    let alive = true;
    setState({ status: 'loading' });
    api
      .readAst(runId, path, line, col, { host })
      .then((response) => {
        if (!alive) return;
        setState({ status: 'loaded', response });
        if (response.root) {
          // `null` means no node anywhere in the tree is matched — fall back
          // to expanding the root so the user never sees a lone collapsed
          // root row with no way to tell there's more beneath it.
          const ancestors = findMatchedAncestorPaths(response.root, '0');
          setExpanded(new Set(ancestors ?? ['0']));
        } else {
          setExpanded(new Set());
        }
      })
      .catch((e) => {
        if (alive) setState({ status: 'error', message: e instanceof Error ? e.message : String(e) });
      });
    return () => {
      alive = false;
    };
  }, [runId, path, line, col, host]);

  const toggle = React.useCallback((p: string) => {
    setExpanded((prev) => {
      const next = new Set(prev);
      if (next.has(p)) next.delete(p);
      else next.add(p);
      return next;
    });
  }, []);

  if (state.status === 'loading') {
    return <div className="text-note text-ink-mute">Loading AST…</div>;
  }

  if (state.status === 'error') {
    return <div className="text-note text-err">Could not load AST: {state.message}</div>;
  }

  const { response } = state;
  if (!response.available || !response.root) {
    return <div className="text-note text-ink-mute">{response.reason ?? 'AST not available.'}</div>;
  }

  return (
    <div className="mt-1 rounded-md border border-border bg-panel p-2 text-[11.5px]">
      <div className="mb-1 flex items-center justify-between gap-2">
        <label className="flex items-center gap-1.5 text-note text-ink-mute">
          <input
            type="checkbox"
            checked={showAnonymous}
            onChange={(e) => setShowAnonymous(e.target.checked)}
          />
          show anonymous
        </label>
        {response.truncated && (
          <span className="text-note text-ink-mute">tree truncated (large file)</span>
        )}
      </div>
      <TreeNode
        node={response.root}
        path="0"
        depth={0}
        expanded={expanded}
        onToggle={toggle}
        showAnonymous={showAnonymous}
      />
    </div>
  );
}
