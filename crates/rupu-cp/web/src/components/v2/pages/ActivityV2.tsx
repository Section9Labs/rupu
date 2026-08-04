// Activity — v2 IA destination merging the v1 run-stream + session list
// pages behind a Segmented tab bar (see Composite.tsx). Interim carrier:
// later plans replace these tab bodies with real v2 page content.
import { lazy } from 'react';
import { Composite } from './Composite';

const AgentRuns = lazy(() => import('../../../pages/runs/AgentRuns'));
const WorkflowRuns = lazy(() => import('../../../pages/runs/WorkflowRuns'));
const AutoflowRuns = lazy(() => import('../../../pages/runs/AutoflowRuns'));
const Sessions = lazy(() => import('../../../pages/Sessions'));

export default function ActivityV2() {
  return (
    <Composite
      title="Activity"
      defaultTab="workflows"
      tabs={[
        { value: 'agents', label: 'agents', element: <AgentRuns /> },
        { value: 'workflows', label: 'workflows', element: <WorkflowRuns /> },
        { value: 'autoflows', label: 'autoflows', element: <AutoflowRuns /> },
        { value: 'sessions', label: 'sessions', element: <Sessions /> },
      ]}
    />
  );
}
