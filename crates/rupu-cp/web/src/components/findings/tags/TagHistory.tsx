// A finding's tag changes, newest first: + / − tag, who (agent codename and
// name, or operator and where), when.
import type { TagActor, TagEvent } from '../../../lib/api';

function who(by: TagActor): string {
  if (by.kind === 'operator') return `${by.user} via ${by.via}`;
  const name = by.codename ?? by.agent ?? 'agent';
  return by.codename && by.agent ? `${name} (${by.agent})` : name;
}

export function TagHistory({ events }: { events: TagEvent[] }) {
  if (events.length === 0) return <p className="text-note text-ink-mute">No tag changes yet.</p>;
  const newest = [...events].reverse();
  return (
    <ul className="space-y-1 text-note">
      {newest.map((e) => (
        <li key={e.id} className="flex flex-wrap items-baseline gap-x-2">
          <span className={e.op === 'add' ? 'font-mono text-ok' : 'font-mono text-err'}>
            {e.op === 'add' ? '+' : '−'} {e.tag}
          </span>
          <span className="text-ink-dim">{who(e.by)}</span>
          <time className="text-ink-mute" dateTime={e.at}>
            {new Date(e.at).toLocaleString()}
          </time>
        </li>
      ))}
    </ul>
  );
}
