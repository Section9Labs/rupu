// BoardRail — the flow's board: claims (held / released / expired), the board
// posts where they landed (each the end of its writer's arc), and the standing
// directives with their retraction. `anchor(key)` puts a connection point in
// an item (the map's arcs land on it): `claim:<key>:<acquired_at>` for a
// claim, the message id for a directive or a post. `compact` lays it out as
// the strip along a workflow frame's foot. Past `limit` items a section
// shows its latest ones under an "N earlier" row that opens it in place
// (`railWindow`, which the map's arc anchors share: an arc to a folded item
// lands on that row).
import type { ReactNode } from 'react';

import { useThemeMode } from '../codename/useThemeMode';
import type { BoardSource, FlowMessage } from '../../lib/flow/flowModel';
import type { ClaimView, DirectiveView } from '../../lib/api';
import { identityOf } from '../../lib/messages';
import { earlierKey, railWindow, type RailSection } from '../../lib/flow/flowEdges';

const NONE_OPEN: ReadonlySet<RailSection> = new Set();

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
  anchor,
  compact = false,
  limit = Infinity,
  open = NONE_OPEN,
  onOpen,
}: {
  board: BoardSource | null;
  posts: FlowMessage[];
  onSelect: (id: string) => void;
  anchor?: (key: string) => ReactNode;
  compact?: boolean;
  /** Items a section shows before folding the rest. */
  limit?: number;
  /** Sections opened past `limit`. */
  open?: ReadonlySet<RailSection>;
  onOpen?: (section: RailSection, open: boolean) => void;
}) {
  const cls = compact ? 'afx-rail afx-rail-compact' : 'afx-rail';
  const mode = useThemeMode();
  const who = (address: string) => (
    <span className="afx-rail-who" style={{ color: identityOf(address, mode).color }}>
      {address}
    </span>
  );

  // A host that can't serve the board still has the posts (they are
  // messages): the note says why claims and directives are missing.
  const unavailable = board && 'kind' in board ? board : null;
  const view = board && !('kind' in board) ? board : null;
  const win = railWindow(board, posts, open, limit);
  // The row a folded section's arcs land on; it opens the section in place,
  // and an opened one offers to fold back.
  const fold = (section: RailSection, earlier: number) => {
    if (earlier > 0) {
      return (
        <button type="button" className="afx-rail-earlier" onClick={() => onOpen?.(section, true)}>
          {anchor?.(earlierKey(section))}
          {earlier} earlier
        </button>
      );
    }
    if (open.has(section) && onOpen) {
      return (
        <button type="button" className="afx-rail-earlier" onClick={() => onOpen(section, false)}>
          latest only
        </button>
      );
    }
    return null;
  };

  return (
    <aside className={cls} aria-label="Board">
      {unavailable && <div className="afx-rail-note">{unavailable.reason || "This host can't show the board."}</div>}
      {board === null && <div className="afx-rail-note">board not loaded.</div>}
      {compact && view && view.claims.length + view.directives.length + posts.length === 0 && (
        <div className="afx-rail-none">
          <h3 className="afx-rail-h">Board</h3> none
        </div>
      )}
      {view && (
        <>
          <section>
            <h3 className="afx-rail-h">Claims</h3>
            {view.claims.length === 0 && <div className="afx-rail-empty">none</div>}
            {fold('claims', win.claims.earlier)}
            {win.claims.items.map((c, i) => (
              <div key={`${c.key}/${c.owner}/${i}`} className="afx-rail-item" data-state={c.state} style={{ ['--i' as string]: i }}>
                {anchor?.(`claim:${c.key}:${c.acquired_at}`)}
                <div className="afx-rail-line">
                  <span className="afx-rail-key">{c.key_known ? c.key : 'claim (key not recorded)'}</span>
                  <span className="afx-rail-state">{c.state}</span>
                </div>
                <div className="afx-rail-sub">
                  {who(c.owner)} · {claimTimes(c)}
                </div>
              </div>
            ))}
            {!view.journal && view.claims.some((c) => !c.key_known) && (
              <div className="afx-rail-note">claims before this version show only while held</div>
            )}
          </section>
          <section>
            <h3 className="afx-rail-h">Directives</h3>
            {view.directives.length === 0 && <div className="afx-rail-empty">none</div>}
            {fold('directives', win.directives.earlier)}
            {win.directives.items.map((d: DirectiveView, i) => (
              <button
                key={d.id}
                type="button"
                className="afx-rail-item afx-rail-btn"
                data-retracted={d.retracted}
                style={{ ['--i' as string]: i }}
                onClick={() => onSelect(d.id)}
              >
                {anchor?.(d.id)}
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
        {fold('posts', win.posts.earlier)}
        {win.posts.items.map((p, i) => (
          <button key={p.id} type="button" className="afx-rail-item afx-rail-btn" style={{ ['--i' as string]: i }} onClick={() => onSelect(p.id)}>
            {anchor?.(p.id)}
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
