import { useEffect, useMemo, useState } from 'react';
import { useSearchParams } from 'react-router-dom';
import { api, type FindingRecord } from '../../lib/api';
import FileTree from '../code/FileTree';
import CodeViewer from '../code/CodeViewer';

export interface ProjectCodeTabProps {
  wsId: string;
}

export default function ProjectCodeTab({ wsId }: ProjectCodeTabProps) {
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
    <div className="grid grid-cols-[minmax(200px,280px)_1fr] gap-3 max-md:grid-cols-1">
      <aside className="max-h-[70vh] overflow-y-auto rounded-md border border-border bg-surface">
        <FileTree wsId={wsId} findings={findings} selectedPath={selected} onSelect={onSelect} />
      </aside>
      <section className="min-w-0">
        {selected ? (
          <CodeViewer
            wsId={wsId}
            path={selected}
            findings={findings}
            initialLine={selected === deepPath ? initialLine : undefined}
          />
        ) : (
          <div className="flex h-40 items-center justify-center text-sm text-ink-dim">
            Select a file to view its source and findings.
          </div>
        )}
      </section>
    </div>
  );
}
