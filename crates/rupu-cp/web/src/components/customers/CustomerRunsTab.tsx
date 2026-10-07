// The customer detail's Runs tab: the Workflow Runs and Agent Runs tables
// (the same components as /runs/workflows and /runs/agents, per-host engine
// and all) with `customer` fixed to the slug. A remote host that can't filter
// by customer answers 501, which the engine reports as an `unavailable` slice
// in the per-host status strip — never as zero runs.

import { useState } from 'react';
import { Segmented } from '../ui/Segmented';
import WorkflowRuns from '../../pages/runs/WorkflowRuns';
import AgentRuns from '../../pages/runs/AgentRuns';

type Kind = 'workflow' | 'agent';

const KIND_OPTIONS = [
  { value: 'workflow', label: 'Workflows' },
  { value: 'agent', label: 'Agents' },
];

export function CustomerRunsTab({ slug }: { slug: string }) {
  const [kind, setKind] = useState<Kind>('workflow');
  return (
    <section className="space-y-3">
      <Segmented
        ariaLabel="Run kind"
        options={KIND_OPTIONS}
        value={kind}
        onChange={(v) => setKind(v as Kind)}
      />
      {/* Keyed by customer and kind: a different customer or kind is a fresh list. */}
      {kind === 'workflow' ? (
        <WorkflowRuns key={`workflow:${slug}`} customer={slug} />
      ) : (
        <AgentRuns key={`agent:${slug}`} customer={slug} />
      )}
    </section>
  );
}
