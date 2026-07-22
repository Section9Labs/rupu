// Full-page Agent Builder — the `next` (card-composer) authoring UI gets the
// whole content area instead of a floating modal. Route: /agents/new (linked
// from the Agents page's "New agent" button when `[cp].agent_authoring_ui`
// resolves to `next`; the classic Describe/Edit modal remains on the default
// path). Cancel returns to /agents; a successful create navigates to the new
// agent's detail page.

import { useEffect, useState } from 'react';
import { useNavigate } from 'react-router-dom';
import { api, type ProviderModels } from '../lib/api';
import AgentBuilder from '../components/agentBuilder/AgentBuilder';
import { NEW_AGENT_TEMPLATE } from '../lib/agentBuilder/agentSpec';

export default function AgentNew() {
  const navigate = useNavigate();
  const [creating, setCreating] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [models, setModels] = useState<ProviderModels[]>([]);
  const [agentNames, setAgentNames] = useState<string[]>([]);

  useEffect(() => {
    let cancelled = false;
    api
      .generateModels()
      .then((m) => {
        if (!cancelled) setModels(m);
      })
      .catch(() => {
        // Non-fatal — the AI tab simply doesn't offer provider choices.
      });
    api
      .getAgents()
      .then((data) => {
        if (!cancelled) setAgentNames(data.map((a) => a.name));
      })
      .catch(() => {
        // Non-fatal — the Dispatch card's agent picker just has no
        // suggestions if this fails.
      });
    return () => {
      cancelled = true;
    };
  }, []);

  async function create(raw: string) {
    if (creating) return;
    setCreating(true);
    setError(null);
    try {
      const created = await api.createAgent(raw);
      navigate(`/agents/${encodeURIComponent(created.name)}`);
    } catch (e: unknown) {
      setError(e instanceof Error ? e.message : 'Failed to create agent');
      setCreating(false);
    }
  }

  return (
    <div className="h-full">
      <AgentBuilder
        initialRaw={NEW_AGENT_TEMPLATE}
        submitLabel="Create agent"
        submitting={creating}
        error={error}
        onSubmit={create}
        onCancel={() => navigate('/agents')}
        aiModels={models}
        onGenerate={api.generateAgent}
        agentNames={agentNames}
      />
    </div>
  );
}
