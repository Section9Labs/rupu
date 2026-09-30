import { Link } from 'react-router-dom';
import Markdown from '../../transcript/Markdown';
import { isSentinel, type CrossRef, type OrSentinel } from '../../../lib/findingReport';

export default function CrossReferences({ refs, references }: { refs: OrSentinel<CrossRef[]>; references: string }) {
  return (
    <div className="space-y-2">
      <div className="text-ink-dim"><Markdown text={references} /></div>
      <div className="text-ui text-ink-dim">
        <span className="font-semibold text-ink">Related findings: </span>
        {isSentinel(refs) ? refs : (
          <ul className="mt-1 space-y-0.5">
            {refs.map((r) => (
              <li key={r.finding_id}>
                <span className="text-ink-mute">{r.relation}</span>{' '}
                <Link to={`/findings/${encodeURIComponent(r.finding_id)}`} className="font-mono text-brand-700 hover:underline">{r.finding_id}</Link>
                {r.note && <span className="text-ink-mute"> — {r.note}</span>}
              </li>
            ))}
          </ul>
        )}
      </div>
    </div>
  );
}
