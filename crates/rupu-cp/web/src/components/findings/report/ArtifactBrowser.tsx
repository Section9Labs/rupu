import { useRef, useState } from 'react';
import { ApiError, apiErrorMessage, findingArtifactUrl } from '../../../lib/api';
import { formatBytes, type ArtifactRef } from '../../../lib/findingReport';

const PREVIEW_LIMIT = 256 * 1024;

/** Server errors are `{"error": "..."}`; show the message, not the JSON. */
async function errorMessage(res: Response): Promise<string> {
  const raw = await res.text().catch(() => '');
  return apiErrorMessage(new ApiError(res.status, raw || res.statusText, raw));
}

export default function ArtifactBrowser({ findingId, artifacts }: { findingId: string; artifacts: ArtifactRef[] }) {
  const [selected, setSelected] = useState<ArtifactRef | null>(null);
  const [text, setText] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  // Bumped on every selection; a fetch only writes state while it is still
  // the latest one, so a slow earlier response can't land under a newer name.
  const token = useRef(0);

  async function open(a: ArtifactRef) {
    const mine = ++token.current;
    const current = () => token.current === mine;
    setSelected(a);
    setText(null);
    setError(null);
    setLoading(false);
    if (a.kind !== 'text' || a.host) return;
    if (a.size > PREVIEW_LIMIT) { setError(`Too large to preview (${formatBytes(a.size)}); download it instead.`); return; }
    setLoading(true);
    try {
      const res = await fetch(findingArtifactUrl(findingId, a.sha256), { credentials: 'same-origin' });
      if (!res.ok) {
        const msg = await errorMessage(res);
        if (current()) setError(msg);
        return;
      }
      const body = await res.text();
      if (current()) setText(body);
    } catch (e) {
      if (current()) setError(apiErrorMessage(e));
    } finally {
      if (current()) setLoading(false);
    }
  }

  return (
    <div className="grid overflow-hidden rounded-md border border-border bg-panel md:grid-cols-[minmax(12rem,16rem)_minmax(0,1fr)]">
      <ul className="border-b border-border p-1.5 md:border-b-0 md:border-r">
        {artifacts.map((a) => {
          // Identity is path AND sha: two paths with identical content share a
          // sha, and must not both read as selected.
          const isSelected = selected?.path === a.path && selected?.sha256 === a.sha256;
          return (
            <li key={`${a.path}-${a.sha256}`}>
              <button
                type="button"
                aria-pressed={isSelected}
                onClick={() => void open(a)}
                className={`flex w-full justify-between gap-2 rounded px-2 py-1 text-left font-mono text-note ${isSelected ? 'bg-brand-50 text-ink' : 'text-ink-dim hover:bg-surface'}`}
              >
                <span className="truncate">{a.path}</span>
                <span className="shrink-0 text-ink-mute">{formatBytes(a.size)}</span>
              </button>
            </li>
          );
        })}
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
            {loading && <p role="status" className="px-3 py-2 text-ui text-ink-mute">Loading…</p>}
            {error && <p role="alert" className="px-3 py-2 text-ui text-err">{error}</p>}
            {text !== null && <pre className="max-h-96 overflow-auto px-3 py-2 text-note font-mono text-ink whitespace-pre">{text}</pre>}
            {selected.kind !== 'text' && !selected.host && <p className="px-3 py-2 text-ui text-ink-mute">Binary file. Use Download.</p>}
          </>
        )}
      </div>
    </div>
  );
}
