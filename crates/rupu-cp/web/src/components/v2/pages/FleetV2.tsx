// Fleet — v2 IA destination merging hosts + workers behind a Segmented tab
// bar (see Composite.tsx). Interim carrier: later plans replace these tab
// bodies with real v2 page content.
import { lazy } from 'react';
import { Composite } from './Composite';

const Hosts = lazy(() => import('../../../pages/Hosts'));
const Workers = lazy(() => import('../../../pages/Workers'));

export default function FleetV2() {
  return (
    <Composite
      title="Fleet"
      defaultTab="hosts"
      tabs={[
        { value: 'hosts', label: 'hosts', element: <Hosts /> },
        { value: 'workers', label: 'workers', element: <Workers /> },
      ]}
    />
  );
}
