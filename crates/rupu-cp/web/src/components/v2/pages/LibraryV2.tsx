// Library — v2 IA destination merging agent/workflow/autoflow definitions
// behind a Segmented tab bar (see Composite.tsx). Interim carrier: later
// plans replace these tab bodies with real v2 page content (a contracts tab
// arrives in Plan 5).
import { lazy } from 'react';
import { Composite } from './Composite';

const Agents = lazy(() => import('../../../pages/Agents'));
const Workflows = lazy(() => import('../../../pages/Workflows'));
const AutoflowsDefs = lazy(() => import('../../../pages/AutoflowsDefs'));

export default function LibraryV2() {
  return (
    <Composite
      title="Library"
      defaultTab="agents"
      tabs={[
        { value: 'agents', label: 'agents', element: <Agents /> },
        { value: 'workflows', label: 'workflows', element: <Workflows /> },
        { value: 'autoflows', label: 'autoflows', element: <AutoflowsDefs /> },
      ]}
    />
  );
}
