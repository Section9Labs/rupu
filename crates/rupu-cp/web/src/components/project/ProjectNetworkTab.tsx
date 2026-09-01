// Project Network tab body — the project-scoped netflow aggregate: every
// flow across every run under this project (workspace-local ledgers plus
// the id-driven global-fallback recovery pass — see
// `rupu_cp::api::netflow::project_scoped_flows_and_dropped`). The scope
// disclosure, fidelity legend and dropped accounting surface through the
// explorer's "Coverage & gaps" popover.
//
// This is the SAME `NetflowExplorer` surface the global page and the run
// Network tab render — scope comes from the route and only narrows the
// data. This tab body only mounts while "Network" is the active
// ProjectDetail tab, so the explorer's mount-time fetch is the lazy-load.
// `key={wsId}` resets all view state (filters, window, zoom stack) when
// the project changes rather than leaking the previous project's filters.

import NetflowExplorer from '../netflow/explorer/NetflowExplorer';

export default function ProjectNetworkTab({ wsId }: { wsId: string }) {
  return <NetflowExplorer key={wsId} scope="project" projectId={wsId} />;
}
