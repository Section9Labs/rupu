// Dispatch card — `dispatchableAgents`, the sub-agents this agent may call
// via dispatch_agent / dispatch_agents_parallel. Suggestions come from
// `agentNames`, threaded down from `AgentBuilderProps` (the shell fetches
// the real list; this card just renders whatever it's given).
import { ChipsInput, LabeledRow } from '../controls';
import type { CardProps } from './types';

export interface DispatchCardProps extends CardProps {
  agentNames?: string[];
}

export default function Dispatch({ draft, patch, agentNames }: DispatchCardProps) {
  const list = draft.dispatchableAgents ?? [];
  return (
    <LabeledRow
      label="Dispatchable agents"
      yamlKey="dispatchableAgents"
      hint="Child agents this one may call via dispatch_agent / dispatch_agents_parallel. Absent = cannot dispatch."
    >
      <ChipsInput
        list={list}
        suggestions={agentNames}
        placeholder="agent name…"
        onChange={(next) => patch({ dispatchableAgents: next })}
      />
    </LabeledRow>
  );
}
