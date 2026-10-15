// The autoflow-run list's query fields (`GET /api/runs/autoflows/runs?q=`):
// every workflow-run field, then the ones only an autoflow row has — the
// issue, the PR, the row's kind (a run, or an attempt that never became
// one) and an attempt's error. The server evaluates them.
import { AlertTriangle, CircleDot, GitPullRequest, Layers } from 'lucide-react';
import { serverField, type ServerField } from '../rows';
import { WORKFLOW_RUN_FIELDS } from './workflowRuns';

export const AUTOFLOW_RUN_FIELDS: ServerField[] = [
  ...WORKFLOW_RUN_FIELDS,
  serverField({ key: 'issue', match: 'contains', text: true, label: 'Issue', description: 'issue:acme/api#7 · issue:widgets (the display ref or the ref, contains)', icon: CircleDot }),
  serverField({ key: 'pr', values: ['any', 'none'], match: 'exact', facet: true, label: 'PR', description: 'pr:any (the run opened one) · pr:none', icon: GitPullRequest }),
  serverField({ key: 'kind', values: ['run', 'attempt'], match: 'exact', facet: true, label: 'Kind', description: 'kind:attempt (failed before a run started) · kind:run', icon: Layers }),
  serverField({ key: 'error', match: 'contains', text: true, label: 'Error', description: 'error:"branch exists" (an attempt’s error, contains)', icon: AlertTriangle }),
];
