// Project Findings tab body — the shared `FindingsExplorer` scoped to one
// project (`GET /api/findings?ws_id=`): the query, tiles, export and tagging
// all stay within the project.

import { FindingsExplorer } from '../findings/FindingsExplorer';

export default function ProjectFindingsTab({ wsId }: { wsId: string }) {
  return (
    <FindingsExplorer
      scope={{ wsId }}
      exportTitle={(rows) => `${rows[0]?.project ?? 'Project'} findings report`}
      exportWsId={wsId}
      tableWsId={wsId}
      empty={{ title: 'No findings', hint: 'No findings for this project.' }}
    />
  );
}
