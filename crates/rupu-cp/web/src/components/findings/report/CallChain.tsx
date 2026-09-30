import { Link } from 'react-router-dom';
import { codeHref, isSentinel, sentinelLabel, type ChainHop, type OrSentinel } from '../../../lib/findingReport';

const DOT: Record<ChainHop['role'], string> = {
  source: 'bg-ink-mute',
  hop: 'bg-brand-500',
  sink: 'bg-sev-critical ring-4 ring-sev-critical/20',
};

export default function CallChain({ chain, wsId }: { chain: OrSentinel<ChainHop[]>; wsId?: string }) {
  if (isSentinel(chain)) return <p className="text-ui text-ink-mute">{sentinelLabel(chain)}</p>;
  return (
    <ol className="space-y-0">
      {chain.map((h, i) => (
        <li key={`${h.label}-${i}`} className="grid grid-cols-[1.25rem_minmax(0,1fr)] gap-3">
          <div className="flex flex-col items-center">
            <span role="img" aria-label={h.role} className={`mt-1.5 h-2.5 w-2.5 rounded-full ${DOT[h.role]}`} />
            {i < chain.length - 1 && <span className="w-px flex-1 bg-border" />}
          </div>
          <div className="min-w-0 space-y-0.5 pb-3">
            <div className={`font-mono text-ui ${h.role === 'sink' ? 'text-sev-critical' : 'text-ink'}`}>{h.label}</div>
            {h.file &&
              (wsId ? (
                <Link to={codeHref(wsId, h.file, h.lines?.[0])} className="font-mono text-note text-brand-700 hover:underline">
                  {h.file}{h.lines ? `:${h.lines[0]}-${h.lines[1]}` : ''}
                </Link>
              ) : (
                <span className="font-mono text-note text-ink-mute">{h.file}{h.lines ? `:${h.lines[0]}-${h.lines[1]}` : ''}</span>
              ))}
            {h.binary_va && <span className="font-mono text-note text-ink-mute">{h.binary_va}</span>}
            {h.gate && (
              <p className="text-ui text-ink-dim">
                Gate: {h.gate}
                {h.passes_because && <> — <span className="text-ok">passes</span>: {h.passes_because}</>}
              </p>
            )}
          </div>
        </li>
      ))}
    </ol>
  );
}
