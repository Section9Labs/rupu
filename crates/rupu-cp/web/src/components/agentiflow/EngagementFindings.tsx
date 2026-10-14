// EngagementFindings — the agentiflow detail's Findings tab: the shared
// `FindingsExplorer` scoped by `GET /api/findings?run_id=<id>` (the server
// unions the lead run with every dispatched sub-run). Polls while the run is
// live.

import { FindingsExplorer } from '../findings/FindingsExplorer';

const POLL_MS = 5000;

export default function EngagementFindings({ runId, live }: { runId: string; live?: boolean }) {
  return (
    <FindingsExplorer
      scope={{ runId }}
      pollMs={live ? POLL_MS : undefined}
      exportTitle="Engagement findings report"
      empty={{
        title: 'No findings recorded yet',
        hint: "A finding appears here once the lead (or a reviewer/assessor) records it with the findings.report tool. The engagement's goal often counts these.",
      }}
    />
  );
}
