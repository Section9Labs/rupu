// BoardRail — the flow's board: claims (held / released / expired), the board
// posts where they landed (each the end of its writer's arc), and the standing
// directives with their retraction.

import { useTheme } from '../theme/ThemeProvider';
import type { BoardSource, FlowMessage } from '../../lib/flow/flowModel';
import type { ClaimView, DirectiveView } from '../../lib/api';
import { identityOf } from '../../lib/messages';

const clock = (iso: string | null | undefined) => {
  const t = iso ? Date.parse(iso) : NaN;
  return Number.isFinite(t) ? new Date(t).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' }) : '—';
};

function claimTimes(c: ClaimView): string {
  switch (c.state) {
    case 'held':
      return `since ${clock(c.acquired_at)}${c.lease_expires_at ? ` · lease to ${clock(c.lease_expires_at)}` : ''}`;
    case 'released':
      return `${clock(c.acquired_at)} – ${clock(c.ended_at)}`;
    case 'expired':
      return `${clock(c.acquired_at)} · expired ${clock(c.ended_at ?? c.lease_expires_at)}`;
  }
}

export default function BoardRail({
  board,
  posts,
  onSelect,
}: {
  board: BoardSource | null;
  posts: FlowMessage[];
  onSelect: (id: string) => void;
}) {
  const { mode } = useTheme();
  const who = (address: string) => (
    <span className="afx-rail-who" style={{ color: identityOf(address, mode).color }}>
      {address}
    </span>
  );

  if (board && 'kind' in board) {
    return (
      <aside className="afx-rail" aria-label="Board">
        <div className="afx-rail-note">
          this host can't show the board.
          {board.reason && <span className="afx-rail-sub"> {board.reason}</span>}
        </div>
      </aside>
    );
  }

  return (
    <aside className="afx-rail" aria-label="Board">
      {board === null && <div className="afx-rail-note">board not loaded.</div>}
      {board && (
        <>
          <section>
            <h3 className="afx-rail-h">Claims</h3>
            {board.claims.length === 0 && <div className="afx-rail-empty">none</div>}
            {board.claims.map((c, i) => (
              <div key={`${c.key}/${c.owner}/${i}`} className="afx-rail-item" data-state={c.state} style={{ ['--i' as string]: i }}>
                <div className="afx-rail-line">
                  <span className="afx-rail-key">{c.key_known ? c.key : 'claim (key not recorded)'}</span>
                  <span className="afx-rail-state">{c.state}</span>
                </div>
                <div className="afx-rail-sub">
                  {who(c.owner)} · {claimTimes(c)}
                </div>
              </div>
            ))}
            {!board.journal && (
              <div className="afx-rail-note">claims before this version show only while held</div>
            )}
          </section>
          <section>
            <h3 className="afx-rail-h">Directives</h3>
            {board.directives.length === 0 && <div className="afx-rail-empty">none</div>}
            {board.directives.map((d: DirectiveView, i) => (
              <button
                key={d.id}
                type="button"
                className="afx-rail-item afx-rail-btn"
                data-retracted={d.retracted}
                style={{ ['--i' as string]: i }}
                onClick={() => onSelect(d.id)}
              >
                <div className="afx-rail-line">
                  {who(d.author)}
                  {d.addressed_to != null && <span className="afx-rail-sub">→ {d.addressed_to}</span>}
                </div>
                <div className="afx-rail-body">{d.body}</div>
                {d.retracted && <div className="afx-rail-sub">retracted {clock(d.retracted_at)}</div>}
              </button>
            ))}
          </section>
        </>
      )}
      <section>
        <h3 className="afx-rail-h">Posts</h3>
        {posts.length === 0 && <div className="afx-rail-empty">none</div>}
        {posts.map((p, i) => (
          <button key={p.id} type="button" className="afx-rail-item afx-rail-btn" style={{ ['--i' as string]: i }} onClick={() => onSelect(p.id)}>
            <div className="afx-rail-line">
              {who(p.message.from)}
              {p.message.post_kind && <span className="afx-rail-sub">{p.message.post_kind}</span>}
            </div>
            <div className="afx-rail-body">{p.body}</div>
          </button>
        ))}
      </section>
    </aside>
  );
}
