import { useState } from 'react';
import { findingArtifactUrl } from '../../../lib/api';
import { formatBytes, type ArtifactRef } from '../../../lib/findingReport';

const PREVIEW_LIMIT = 256 * 1024;

export default function ArtifactBrowser({ findingId, artifacts }: { findingId: string; artifacts: ArtifactRef[] }) {
  const [selected, setSelected] = useState<ArtifactRef | null>(null);
  const [text, setText] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  async function open(a: ArtifactRef) {
    setSelected(a);
    setText(null);
    setError(null);
    if (a.kind !== 'text' || a.host) return;
    if (a.size > PREVIEW_LIMIT) { setError(`Too large to preview (${formatBytes(a.size)}); download it instead.`); return; }
    try {
      const res = await fetch(findingArtifactUrl(findingId, a.sha256), { credentials: 'same-origin' });
      if (!res.ok) { setError(await res.text()); return; }
      setText(await res.text());
    } catch (e) {
      setError(String(e));
    }
  }

  return (
    <div className="grid overflow-hidden rounded-md border border-border bg-panel md:grid-cols-[minmax(12rem,16rem)_minmax(0,1fr)]">
      <ul className="border-b border-border p-1.5 md:border-b-0 md:border-r">
        {artifacts.map((a) => (
          <li key={`${a.path}-${a.sha256}`}>
            <button
              type="button"
              onClick={() => void open(a)}
              className={`flex w-full justify-between gap-2 rounded px-2 py-1 text-left font-mono text-note ${selected?.sha256 === a.sha256 ? 'bg-brand-50 text-ink' : 'text-ink-dim hover:bg-surface'}`}
            >
              <span className="truncate">{a.path}</span>
              <span className="shrink-0 text-ink-mute">{formatBytes(a.size)}</span>
            </button>
          </li>
        ))}
      </ul>
      <div className="min-w-0">
        {!selected && <p className="px-3 py-2 text-ui text-ink-mute">Select an artifact to preview it.</p>}
        {selected && (
          <>
            <div className="flex flex-wrap items-center justify-between gap-2 border-b border-border px-3 py-1.5 font-mono text-note text-ink-mute">
              <span className="truncate">{selected.path}</span>
              {selected.host ? (
                <span>stored on host {selected.host}</span>
              ) : (
                <a href={findingArtifactUrl(findingId, selected.sha256)} download className="text-brand-700 hover:underline">Download</a>
              )}
            </div>
            {error && <p className="px-3 py-2 text-ui text-err">{error}</p>}
            {text !== null && <pre className="max-h-96 overflow-auto px-3 py-2 text-note font-mono text-ink whitespace-pre">{text}</pre>}
            {selected.kind !== 'text' && !selected.host && <p className="px-3 py-2 text-ui text-ink-mute">Binary file. Use Download.</p>}
          </>
        )}
      </div>
    </div>
  );
}
