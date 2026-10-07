import { Link } from 'react-router-dom';
import Markdown from '../../transcript/Markdown';
import { isSentinel, sentinelLabel, type CrossRef, type OrSentinel } from '../../../lib/findingReport';
import { findingPath, useFindingWorkspace } from '../../../lib/findingWorkspace';

export default function CrossReferences({ refs, references }: { refs: OrSentinel<CrossRef[]>; references: string }) {
  // A cross-reference names a finding in the same project.
  const wsId = useFindingWorkspace();
  return (
    <div className="space-y-2">
      <div className="text-ink-dim"><Markdown text={sentinelLabel(references)} /></div>
      <div className="text-ui text-ink-dim">
        <span className="font-semibold text-ink">Related findings: </span>
        {isSentinel(refs) ? sentinelLabel(refs) : (
          <ul className="mt-1 space-y-0.5">
            {refs.map((r, i) => (
              <li key={`${r.finding_id}-${r.relation}-${i}`}>
                <span className="text-ink-mute">{r.relation}</span>{' '}
                <Link to={findingPath(r.finding_id, wsId)} className="font-mono text-brand-700 hover:underline">{r.finding_id}</Link>
                {r.note && <span className="text-ink-mute"> — {r.note}</span>}
              </li>
            ))}
          </ul>
        )}
      </div>
    </div>
  );
}
