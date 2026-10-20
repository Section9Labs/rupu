// ContinueComposer — the composer under a standalone agent run's transcript
// (mock §2). A continuation restores every flag of the original launch, so the
// composer asks only for what can change: an optional prompt (what the run is
// told next, `rupu run --continue <id> --prompt=…`) and a different model or
// provider, under a disclosure that names the original ones. The CLI's rules
// decide what can go: a completed run needs a prompt (without one a continue
// only reprints its answer); a failed run needs a prompt or another model or
// provider; an interrupted one needs nothing. On success it links to the new
// run.

import { useEffect, useId, useState } from 'react';
import { Link } from 'react-router-dom';
import { ChevronRight } from 'lucide-react';
import { api, apiErrorMessage, type CatalogProvider } from '../../lib/api';
import { runsApi, type ContinueResponse } from '../../lib/api3b/runs';
import { Button } from '../ui/Button';
import { Input, inputCls } from '../ui/Input';
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
  status,
  onCancel,
}: {
  runId: string;
  /** The run's transcript path. */
  path: string;
  /** The provider and model the run used, when known. */
  provider?: string | null;
  model?: string | null;
  /** How the run ended: `completed`, `failed`, or anything else (cancelled,
   *  or no status for a run that crashed before recording an end). */
  status: string | null;
  onCancel: () => void;
}) {
  const completed = status === 'completed';
  const failed = status === 'failed';
  const [open, setOpen] = useState(failed);
  const [prompt, setPrompt] = useState('');
  const [newProvider, setNewProvider] = useState('');
  const [newModel, setNewModel] = useState('');
  const [catalog, setCatalog] = useState<CatalogProvider[]>([]);
  const [sending, setSending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [done, setDone] = useState<ContinueResponse | null>(null);
  const providerId = useId();
  const modelId = useId();
  const modelsId = useId();
  const promptId = useId();
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
  const prompted = prompt.trim() !== '';
  // A finished run with no prompt would only reprint its answer; a failed
  // one with neither a prompt nor another model would be refused.
  const blocked = (completed && !prompted) || (failed && !prompted && !changed);
  const original = [provider, model].filter(Boolean).join(' · ');
  const modelsFor = catalog.find((c) => c.provider === (newProvider || provider))?.models ?? [];

  async function submit() {
    if (sending || blocked) return;
    setSending(true);
    setError(null);
    try {
      const res = await runsApi.continueAgentRun(runId, {
        prompt: prompted ? prompt : undefined,
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
        {completed
          ? 'Carries the finished conversation on, with every flag of its original launch. A finished run needs a prompt: without one, a continue only reprints its answer.'
          : 'Picks up where the run stopped, with every flag of its original launch.'}
        {failed && ' A failed run continues with a prompt, or on a different model or provider.'}
      </p>

      <label htmlFor={promptId} className="mt-3 block text-note font-semibold text-ink">
        Prompt{!completed && <span className="font-normal text-ink-mute"> (optional)</span>}
      </label>
      <textarea
        id={promptId}
        value={prompt}
        onChange={(e) => setPrompt(e.target.value)}
        required={completed}
        rows={3}
        placeholder={
          completed
            ? 'What should the run do next?'
            : 'Optional — what to tell the run next. Leave empty to carry on as it was.'
        }
        className={cn(inputCls, 'mt-1 max-w-none resize-y text-ui leading-relaxed')}
      />

      {open && (
        <div className="mt-3 grid gap-3 sm:grid-cols-2">
          <div>
            <label htmlFor={providerId} className="block text-note font-semibold text-ink">
              Provider
            </label>
            <Select
              id={providerId}
              value={newProvider}
              onChange={(e) => {
                // Another provider brings its own first model: the run's model
                // belongs to the old provider. Back to "Same" clears both.
                const next = e.target.value;
                setNewProvider(next);
                setNewModel(next ? (catalog.find((c) => c.provider === next)?.models[0]?.id ?? '') : '');
              }}
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
