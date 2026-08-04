// Security — v2 IA destination merging findings + coverage + the coverage
// template catalog behind a Segmented tab bar (see Composite.tsx). Interim
// carrier: later plans replace these tab bodies with real v2 page content.
import { lazy } from 'react';
import { Composite } from './Composite';

const Findings = lazy(() => import('../../../pages/Findings'));
const Coverage = lazy(() => import('../../../pages/Coverage'));
const CoverageTemplates = lazy(() => import('../../../pages/CoverageTemplates'));

export default function SecurityV2() {
  return (
    <Composite
      title="Security"
      defaultTab="findings"
      tabs={[
        { value: 'findings', label: 'findings', element: <Findings /> },
        { value: 'coverage', label: 'coverage', element: <Coverage /> },
        { value: 'catalog', label: 'catalog', element: <CoverageTemplates /> },
      ]}
    />
  );
}
