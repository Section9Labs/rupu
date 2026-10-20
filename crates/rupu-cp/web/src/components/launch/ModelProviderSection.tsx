// ModelProviderSection — the agent launcher's collapsed "Model & provider"
// section (Plan 3b mock §5). The selects default to the agent's own provider
// and model (the empty value: nothing is sent, the agent file decides); the
// lists are `GET /api/models`. Choosing another provider picks its first
// model, so a launch never pairs a provider with a model it doesn't serve.

import { useEffect, useState } from 'react';
import { api, apiErrorMessage, type CatalogProvider } from '../../lib/api';
import { Select } from '../ui/Select';

/** `''` is "the agent's own". */
export interface ModelChoice {
  provider: string;
  model: string;
}

export default function ModelProviderSection({
  agentProvider,
  agentModel,
  value,
  onChange,
  disabledReason,
  disabled = false,
}: {
  agentProvider?: string | null;
  agentModel?: string | null;
  value: ModelChoice;
  onChange: (c: ModelChoice) => void;
  disabledReason: string | null;
  disabled?: boolean;
}) {
  const [catalog, setCatalog] = useState<CatalogProvider[] | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    api
      .getModelCatalog()
      .then((c) => {
        if (!cancelled) setCatalog(c);
      })
      .catch((e: unknown) => {
        if (!cancelled) setLoadError(apiErrorMessage(e));
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const off = disabled || disabledReason !== null;
  const others = (catalog ?? []).filter((p) => p.provider !== agentProvider);
  const modelsOf = (provider: string) =>
    (catalog ?? []).find((p) => p.provider === provider)?.models.map((m) => m.id) ?? [];
  const modelOptions = value.provider
    ? modelsOf(value.provider)
    : modelsOf(agentProvider ?? '').filter((m) => m !== agentModel);
  const changed = value.provider !== '' || value.model !== '';

  return (
    <details className="group" data-testid="launch-model">
      <summary className="cursor-pointer list-none text-ui font-semibold uppercase tracking-wide text-ink-dim hover:text-ink">
        <span className="group-open:hidden">▸ Model &amp; provider</span>
        <span className="hidden group-open:inline">▾ Model &amp; provider</span>
        <span className="ml-1 normal-case tracking-normal text-ink-mute">
          {changed
            ? `(${value.provider || agentProvider || 'agent provider'} · ${value.model || agentModel || 'agent model'})`
            : '(agent default)'}
        </span>
      </summary>
      <div className="mt-2 space-y-1.5">
        <div className="grid grid-cols-2 gap-2">
          <Select
            value={value.provider}
            onChange={(e) => {
              const provider = e.target.value;
              onChange({ provider, model: provider ? (modelsOf(provider)[0] ?? '') : '' });
            }}
            disabled={off}
            aria-label="Provider"
            className="w-full"
          >
            <option value="">{agentProvider ? `${agentProvider} (agent default)` : 'Agent default'}</option>
            {others.map((p) => (
              <option key={p.provider} value={p.provider}>
                {p.provider}
              </option>
            ))}
          </Select>
          <Select
            value={value.model}
            onChange={(e) => onChange({ ...value, model: e.target.value })}
            disabled={off}
            aria-label="Model"
            className="w-full"
          >
            {value.provider === '' && (
              <option value="">{agentModel ? `${agentModel} (agent default)` : 'Agent default'}</option>
            )}
            {modelOptions.map((m) => (
              <option key={m} value={m}>
                {m}
              </option>
            ))}
          </Select>
        </div>
        {disabledReason !== null && <p className="text-note text-ink-mute">{disabledReason}</p>}
        {loadError && <p className="text-note text-ink-mute">Model list unavailable: {loadError}</p>}
      </div>
    </details>
  );
}
