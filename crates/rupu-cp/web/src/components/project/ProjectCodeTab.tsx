import { useEffect, useMemo, useState } from 'react';
import { useSearchParams } from 'react-router-dom';
import { api, type FindingRecord } from '../../lib/api';
import FileNavigator from '../code/FileNavigator';
import CodeViewer from '../code/CodeViewer';

export interface ProjectCodeTabProps {
  wsId: string;
  /** Repository landing-page URL (github/gitlab), when the workspace's
   *  remote resolves to a known host. Renders a "View on repository" link
   *  in the header above the two-pane grid. */
  repoHomeUrl?: string | null;
  /** Raw `git remote get-url origin` value — displayed (shortened to
   *  `owner/repo`) in the repo chip even when `repoHomeUrl` is unset. */
  repoRemote?: string | null;
  /** Branch the workspace was registered against — shown alongside the repo
   *  chip. */
  branch?: string | null;
}

export default function ProjectCodeTab({
  wsId,
  repoHomeUrl,
  repoRemote,
  branch,
}: ProjectCodeTabProps) {
  const [params, setParams] = useSearchParams();
  const deepPath = params.get('path');
  const deepLine = params.get('line');
  const [selected, setSelected] = useState<string | null>(deepPath);
  const [findings, setFindings] = useState<FindingRecord[]>([]);

  // Findings for the whole project (badges + inline cards).
  useEffect(() => {
    let live = true;
    api
      .getFindings({ wsId })
      .then((r) => live && setFindings(r.findings as unknown as FindingRecord[]))
      .catch(() => live && setFindings([]));
    return () => {
      live = false;
    };
  }, [wsId]);

  // Keep the selection in sync with the URL deep-link.
  useEffect(() => {
    if (deepPath) setSelected(deepPath);
  }, [deepPath]);

  const initialLine = useMemo(() => (deepLine ? Number(deepLine) : undefined), [deepLine]);

  const onSelect = (path: string) => {
    setSelected(path);
    // Reflect selection in the URL (drop the line anchor on manual browse).
    setParams({ path }, { replace: true });
  };

  return (
    <div>
      {(repoHomeUrl || repoRemote) && (
        <div className="mb-2 flex items-center gap-2 text-[12px] text-ink-dim">
          <span className="rounded bg-surface px-2 py-0.5 font-mono">
            {repoRemote?.replace(/^.*[:/]([^/]+\/[^/]+?)(?:\.git)?$/, '$1') ?? 'repo'}
            {branch ? ` · ${branch}` : ''}
          </span>
          {repoHomeUrl && (
            <a
              href={repoHomeUrl}
              target="_blank"
              rel="noreferrer"
              className="text-brand-700 hover:underline"
            >
              View on repository ↗
            </a>
          )}
        </div>
      )}
      {/* Flex (not grid) so the fixed container height propagates to the panes:
          a grid with only `grid-cols` gives an auto (content-height) row, so
          `h-full` children collapse to content and the code view grows to the
          whole file instead of scrolling. Flex items stretch to the container
          height by default, so each pane fills the viewport and scrolls
          internally like an editor. The `12rem` offset (app header + project
          header + tab bar) is an estimate — tune if it clips or leaves a gap. */}
      <div className="flex h-[calc(100vh-12rem)] min-h-[480px] gap-3 max-md:h-auto max-md:flex-col">
        <aside className="h-full w-[264px] shrink-0 overflow-y-auto rounded-md border border-border bg-surface max-md:h-64 max-md:w-full">
          <FileNavigator wsId={wsId} findings={findings} selectedPath={selected} onSelect={onSelect} />
        </aside>
        <section className="h-full min-w-0 flex-1">
          {selected ? (
            <CodeViewer
              wsId={wsId}
              path={selected}
              findings={findings}
              initialLine={selected === deepPath ? initialLine : undefined}
            />
          ) : (
            <div className="flex h-full items-center justify-center rounded-md border border-border bg-panel text-sm text-ink-dim">
              Select a file to view its source and findings.
            </div>
          )}
        </section>
      </div>
    </div>
  );
}
