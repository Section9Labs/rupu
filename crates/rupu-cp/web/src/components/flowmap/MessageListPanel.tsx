// MessageListPanel — every message of every loaded space, in time order,
// tagged by space, kind and participants. Selecting a row selects its arc.

import { useTheme } from '../theme/ThemeProvider';
import { useThemeColors } from '../../lib/useThemeColors';
import { messagePaint } from '../../lib/flow/messagePaint';
import type { FlowMessage } from '../../lib/flow/flowModel';
import { identityOf } from '../../lib/messages';

const clock = (ms: number) => new Date(ms).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', second: '2-digit' });

function recipients(m: FlowMessage): string {
  const d = m.message.delivery;
  const to = d?.resolved?.length ? d.resolved : [m.message.to ?? 'board'];
  return to.join(', ');
}

export default function MessageListPanel({
  messages,
  spaces,
  selected,
  onSelect,
}: {
  messages: FlowMessage[];
  /** Space id -> the label its tag shows. */
  spaces: Map<string, string>;
  selected: string | null;
  onSelect: (id: string | null) => void;
}) {
  const colors = useThemeColors();
  const { mode } = useTheme();
  const rows = [...messages].sort((a, b) => a.at - b.at);
  if (rows.length === 0) return <div className="afx-list-empty">No messages yet.</div>;
  return (
    <div className="afx-list" role="listbox" aria-label="Messages">
      {rows.map((m) => {
        const from = m.message.from_codename ?? m.message.from;
        const on = m.id === selected;
        return (
          <button
            key={`${m.space}/${m.id}`}
            type="button"
            role="option"
            aria-selected={on}
            data-testid={`msg-row-${m.id}`}
            className="afx-list-row"
            data-selected={on}
            onClick={() => onSelect(on ? null : m.id)}
          >
            <span className="afx-list-meta">
              <span className="afx-list-time">{m.atKnown ? clock(m.at) : '—'}</span>
              <span className="afx-list-tag">{spaces.get(m.space) ?? m.space}</span>
              <span className="afx-list-tag" style={{ color: messagePaint(m.kind, colors).stroke }}>
                {m.kind}
              </span>
              {m.undelivered.length > 0 && <span className="afx-list-tag afx-list-tag-dead">undelivered</span>}
            </span>
            <span className="afx-list-who">
              <span style={{ color: identityOf(m.message.from, mode).color }}>{from}</span>
              <span aria-hidden> → </span>
              <span>{recipients(m)}</span>
            </span>
            <span className="afx-list-body">{m.body}</span>
          </button>
        );
      })}
    </div>
  );
}
