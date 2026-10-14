// The Workflows library's query fields.
import { CalendarClock, CircleCheck, FolderGit2, History, Timer, Workflow } from 'lucide-react';
import type { WorkflowSummary } from '../../api';
import { rowField, type RowField } from '../rows';

function autoflowState(w: WorkflowSummary): string {
  if (w.autoflow_enabled === null || w.autoflow_enabled === undefined) return 'none';
  return w.autoflow_enabled ? 'enabled' : 'disabled';
}

export const WORKFLOW_FIELDS: RowField<WorkflowSummary>[] = [
  rowField({ key: 'name', match: 'contains', get: (w) => w.name, text: true, label: 'Name', description: 'name:review (contains)', icon: Workflow }),
  rowField({ key: 'scope', aliases: ['project'], match: 'exact', get: (w) => w.scope, facet: true, label: 'Scope', description: 'scope:global · scope:<project>', icon: FolderGit2 }),
  rowField({ key: 'autoflow', values: ['enabled', 'disabled', 'none'], match: 'exact', get: autoflowState, facet: true, label: 'Autoflow', description: 'autoflow:enabled · -autoflow:none (every autoflow)', icon: Timer }),
  rowField({ key: 'has', values: ['runs'], match: 'exact', get: (w) => (w.run_count > 0 ? ['runs'] : []), facet: true, label: 'Has', description: 'has:runs · -has:runs (never ran)', icon: CircleCheck }),
  rowField({ key: 'since', match: 'since', get: (w) => w.last_run, label: 'Ran since', description: 'since:7d · since:2026-10-01 (last run)', icon: History }),
  rowField({ key: 'until', match: 'until', get: (w) => w.last_run, label: 'Ran until', description: 'until:30d (last run before)', icon: CalendarClock }),
];
