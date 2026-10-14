// The Agents list's query fields.
import { Bot, CalendarClock, CircleCheck, Cpu, FolderGit2, Gauge, History, Server, Sparkles, Wrench } from 'lucide-react';
import type { AgentSummary } from '../../api';
import { rowField, type RowField } from '../rows';

export const AGENT_FLAGS = ['load_error', 'skills', 'tools', 'runs', 'description'] as const;

function agentFlags(a: AgentSummary): string[] {
  const out: string[] = [];
  if (a.load_error) out.push('load_error');
  if ((a.skills?.length ?? 0) + (a.preload_skills?.length ?? 0) > 0) out.push('skills');
  if ((a.tools?.length ?? 0) > 0) out.push('tools');
  if (a.run_count > 0) out.push('runs');
  if (a.description) out.push('description');
  return out;
}

export const AGENT_FIELDS: RowField<AgentSummary>[] = [
  rowField({ key: 'name', match: 'contains', get: (a) => [a.name, a.slug], text: true, label: 'Name', description: 'name:recon (contains)', icon: Bot }),
  rowField({ key: 'provider', match: 'exact', get: (a) => a.provider, facet: true, label: 'Provider', description: 'provider:anthropic', icon: Server }),
  rowField({ key: 'model', match: 'exact', get: (a) => a.model, facet: true, label: 'Model', description: 'model:claude-opus-4-1', icon: Cpu }),
  rowField({ key: 'effort', match: 'exact', get: (a) => a.effort, facet: true, label: 'Effort', description: 'effort:high', icon: Gauge }),
  rowField({ key: 'scope', aliases: ['project'], match: 'exact', get: (a) => a.scope, facet: true, label: 'Scope', description: 'scope:global · scope:<project>', icon: FolderGit2 }),
  rowField({ key: 'tool', match: 'exact', get: (a) => a.tools, facet: true, label: 'Tool', description: 'tool:bash (in its tools:)', icon: Wrench }),
  rowField({ key: 'skill', match: 'exact', get: (a) => [...(a.skills ?? []), ...(a.preload_skills ?? [])], facet: true, label: 'Skill', description: 'skill:nmap (skills: or preloadSkills:)', icon: Sparkles }),
  rowField({ key: 'has', values: [...AGENT_FLAGS], match: 'exact', get: agentFlags, facet: true, label: 'Has', description: 'has:load_error · -has:runs (never ran)', icon: CircleCheck }),
  rowField({ key: 'since', match: 'since', get: (a) => a.last_run, label: 'Ran since', description: 'since:7d · since:2026-10-01 (last run)', icon: History }),
  rowField({ key: 'until', match: 'until', get: (a) => a.last_run, label: 'Ran until', description: 'until:30d (last run before)', icon: CalendarClock }),
  rowField({ key: 'description', aliases: ['desc'], match: 'contains', get: (a) => a.description, text: true, label: 'Description', description: 'description:triage', icon: Bot }),
];
