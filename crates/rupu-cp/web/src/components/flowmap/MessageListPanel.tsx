// MessageListPanel — every message of every loaded space, in time order,
// tagged by space, kind and participants. Selecting a row selects its arc.
// Memoized (a replay frame, a card hover or a poll that changes none of its
// props renders nothing here); past WINDOW_THRESHOLD rows only the rows near
// the viewport are mounted (`useWindowVirtualRows`, as the Messages feed).

import { memo, useMemo } from 'react';
import { useThemeMode } from '../codename/useThemeMode';
import { useThemeColors } from '../../lib/useThemeColors';
import { messagePaint } from '../../lib/flow/messagePaint';
import type { FlowMessage } from '../../lib/flow/flowModel';
import { identityOf } from '../../lib/messages';
import { useWindowVirtualRows } from '../lists/useWindowVirtualRows';

/** Rows past which the list mounts only the ones near the viewport. */
export const WINDOW_THRESHOLD = 300;

const clock = (ms: number) => new Date(ms).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', second: '2-digit' });

function recipients(m: FlowMessage): string {
  const d = m.message.delivery;
  const to = d?.resolved?.length ? d.resolved : [m.message.to ?? 'board'];
  return to.join(', ');
}

const keyOf = (m: FlowMessage) => `${m.space}/${m.id}`;

function MessageListPanel({
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
  const mode = useThemeMode();
  const rows = useMemo(() => [...messages].sort((a, b) => a.at - b.at), [messages]);
  const keys = useMemo(() => rows.map(keyOf), [rows]);
  const pin = useMemo(() => {
    const m = selected ? rows.find((r) => r.id === selected) : undefined;
    return m ? keyOf(m) : null;
  }, [rows, selected]);
  const win = useWindowVirtualRows<HTMLDivElement, HTMLButtonElement>(keys, keys.length > WINDOW_THRESHOLD, pin);
  if (rows.length === 0) return <div className="afx-list-empty">No messages yet.</div>;
  const row = (m: FlowMessage) => {
    const from = m.message.from_codename ?? m.message.from;
    const on = m.id === selected;
    const key = keyOf(m);
    return (
      <button
        key={key}
        ref={win.measure(key)}
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
  };
  return (
    <div className="afx-list" role="listbox" aria-label="Messages" ref={win.bodyRef}>
      {win.slots.map((slot) =>
        slot.kind === 'spacer' ? (
          <div key={slot.key} aria-hidden style={{ height: slot.px, flex: 'none' }} />
        ) : (
          rows.slice(slot.start, slot.end).map(row)
        ),
      )}
    </div>
  );
}

export default memo(MessageListPanel);
