// ContinueComposer — the composer under a standalone agent run's transcript
// (mock §2). A continuation restores every flag of the original launch, so the
// composer asks only for what can change: a different model or provider,
// under a disclosure that names the original ones. The CP's continue takes no
// prompt (the conversation already holds it; Plan 3a package B), so there is
// no text box. A failed run is continued only on a different model or
// provider, as the CLI rules. On success it links to the new run.

import { useEffect, useId, useState } from 'react';
import { Link } from 'react-router-dom';
import { ChevronRight } from 'lucide-react';
import { api, apiErrorMessage, type CatalogProvider } from '../../lib/api';
import { runsApi, type ContinueResponse } from '../../lib/api3b/runs';
import { Button } from '../ui/Button';
import { Input } from '../ui/Input';
import { Select } from '../ui/Select';
import { cn } from '../../lib/cn';

/** The transcript page of a run started by a continue: the new run writes its
 *  transcript next to the one it continues. */
export function continuedTranscriptHref(path: string, newRunId: string): string {
  const dir = path.slice(0, path.lastIndexOf('/') + 1);
  return `/transcript?path=${encodeURIComponent(`${dir}${newRunId}.jsonl`)}&live=1`;
}

export function ContinueComposer({
  runId,
  path,
  provider,
  model,
  failed,
  onCancel,
}: {
  runId: string;
  /** The run's transcript path. */
  path: string;
  /** The provider and model the run used, when known. */
  provider?: string | null;
  model?: string | null;
  failed: boolean;
  onCancel: () => void;
}) {
  const [open, setOpen] = useState(failed);
  const [newProvider, setNewProvider] = useState('');
  const [newModel, setNewModel] = useState('');
  const [catalog, setCatalog] = useState<CatalogProvider[]>([]);
  const [sending, setSending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [done, setDone] = useState<ContinueResponse | null>(null);
  const providerId = useId();
  const modelId = useId();
  const modelsId = useId();
  const titleId = useId();

  useEffect(() => {
    let cancelled = false;
    api
      .getModelCatalog()
      .then((c) => {
        if (!cancelled) setCatalog(c);
      })
      .catch(() => {
        /* no catalog → free-text model, original provider only */
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const changed = newProvider.trim() !== '' || newModel.trim() !== '';
  const blocked = failed && !changed;
  const original = [provider, model].filter(Boolean).join(' · ');
  const modelsFor = catalog.find((c) => c.provider === (newProvider || provider))?.models ?? [];

  async function submit() {
    if (sending || blocked) return;
    setSending(true);
    setError(null);
    try {
      const res = await runsApi.continueAgentRun(runId, {
        model: newModel.trim() || undefined,
        provider: newProvider.trim() || undefined,
      });
      setDone(res);
    } catch (e: unknown) {
      setError(apiErrorMessage(e));
    } finally {
      setSending(false);
    }
  }

  if (done) {
    return (
      <div className="rounded-xl border border-border bg-panel px-4 py-3 text-ui text-ink-dim">
        <Link to={continuedTranscriptHref(path, done.run_id)} className="font-medium text-ink hover:underline">
          Continued as <span className="font-mono">{done.run_id}</span> →
        </Link>
      </div>
    );
  }

  return (
    <section
      aria-labelledby={titleId}
      className="rounded-xl border border-border-strong bg-surface px-3.5 py-3"
      onKeyDown={(e) => {
        if ((e.metaKey || e.ctrlKey) && e.key === 'Enter') {
          e.preventDefault();
          void submit();
        }
      }}
    >
      <h2 id={titleId} className="sr-only">
        Continue this run
      </h2>
      <p className="text-ui text-ink-dim">
        Picks up where the run stopped, with every flag of its original launch.
        {failed && ' A failed run continues only on a different model or provider.'}
      </p>

      {open && (
        <div className="mt-3 grid gap-3 sm:grid-cols-2">
          <div>
            <label htmlFor={providerId} className="block text-note font-semibold text-ink">
              Provider
            </label>
            <Select
              id={providerId}
              value={newProvider}
              onChange={(e) => setNewProvider(e.target.value)}
              className="mt-1 w-full text-ui"
            >
              <option value="">{provider ? `Same (${provider})` : 'Same as the original'}</option>
              {catalog
                .filter((c) => c.provider !== provider)
                .map((c) => (
                  <option key={c.provider} value={c.provider}>
                    {c.provider}
                  </option>
                ))}
            </Select>
          </div>
          <div>
            <label htmlFor={modelId} className="block text-note font-semibold text-ink">
              Model
            </label>
            <Input
              id={modelId}
              value={newModel}
              onChange={(e) => setNewModel(e.target.value)}
              list={modelsId}
              placeholder={model ? `Same (${model})` : 'Same as the original'}
              className="mt-1 max-w-none font-mono text-ui"
            />
            <datalist id={modelsId}>
              {modelsFor.map((m) => (
                <option key={m.id} value={m.id} />
              ))}
            </datalist>
          </div>
        </div>
      )}

      {error && (
        <p role="alert" className="mt-2 text-ui font-medium text-err">
          {error}
        </p>
      )}

      <div className="mt-3 flex flex-wrap items-center gap-2">
        <button
          type="button"
          aria-expanded={open}
          onClick={() => setOpen((v) => !v)}
          className="inline-flex items-center gap-1 text-note text-ink-dim hover:text-ink"
        >
          <ChevronRight size={13} aria-hidden className={cn('transition-transform', open && 'rotate-90')} />
          Model &amp; provider
          {original && !changed && (
            <span className="text-ink-mute">(same as the original: {original})</span>
          )}
        </button>
        <span className="flex-1" />
        <kbd className="rounded border border-border-strong px-1 font-mono text-meta text-ink-mute">⌘↵</kbd>
        <Button variant="secondary" onClick={onCancel}>
          Cancel
        </Button>
        <Button variant="solid" disabled={sending || blocked} onClick={() => void submit()}>
          {sending ? 'Continuing…' : 'Continue run'}
        </Button>
      </div>
    </section>
  );
}

export default ContinueComposer;
