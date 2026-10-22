// The Needs-you deck, pinned above the event stream: gates that wait on you,
// unseen failures, unseen critical and high findings — one stack, the most
// urgent card in front and the rest peeking behind it (scaled .96/.92, 7px
// each). Hover, or "+N more", fans it into a list over a veil with a short
// staggered spring; it folds back the reverse way. Approve, Reject or Seen
// sends the card home: the rail morphs a shell of it onto its row (see
// EventRail's sendHome) and the deck folds. The deck is the same item as the
// row — answering either answers both.

import { useLayoutEffect, useRef, useState, type CSSProperties } from 'react';
import { Check, ChevronDown, CornerDownRight } from 'lucide-react';
import type { StreamCard } from '../../lib/situationRoom/cards';
import { ctxOf, deckBrief, markOf, type RailScope, type Resolved } from '../../lib/events/rail';
import { Ctx } from './EventRow';
import GateButtons, { type EventGate } from './GateButtons';
import { EvCap, EvGlyph, Rolling } from './parts';

const PEEK = 7;
const hoverable = () => typeof window !== 'undefined' && !!window.matchMedia?.('(hover: hover) and (pointer: fine)').matches;

export interface DeckProps {
  items: StreamCard[];
  scope: RailScope;
  resolve: (card: StreamCard) => Resolved;
  gate?: EventGate;
  onSeen: (card: StreamCard, el: HTMLElement | null) => void;
  /** About to answer a gate from the deck: the rail keeps the card's shell to send home. */
  onAnswer: (card: StreamCard, el: HTMLElement | null) => void;
  onOpen: (card: StreamCard) => void;
  onKin: (thread: string | null) => void;
  crewOf: (card: StreamCard) => string | undefined;
}

export default function NeedsYouDeck({ items, scope, resolve, gate, onSeen, onAnswer, onOpen, onKin, crewOf }: DeckProps) {
  const [open, setOpen] = useState(false);
  const [pinned, setPinned] = useState(false);
  const deckRef = useRef<HTMLDivElement | null>(null);
  const stackRef = useRef<HTMLDivElement | null>(null);
  const known = useRef<Set<string> | null>(null);
  const closeT = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const prevFront = useRef<string | undefined>(undefined);

  // Cards that join after the deck first showed drop in from above (and keep
  // the mark, so a later render never swaps their animation and replays it).
  const arrivedRef = useRef(new Set<string>());
  const arrived = arrivedRef.current;
  if (known.current) for (const c of items) if (!known.current.has(c.key)) arrived.add(c.key);

  const n = items.length;
  const isOpen = open && n > 1;

  // Lay the stack out from measured heights: a peek's bottom edge shows PEEK
  // px per level below the front card, whatever its own height.
  useLayoutEffect(() => {
    known.current = new Set([...(known.current ?? []), ...items.map((c) => c.key)]);
    const stack = stackRef.current;
    const deck = deckRef.current;
    if (!stack || !deck) return;
    const layout = () => {
      const cards = [...stack.children] as HTMLElement[];
      if (cards.length === 0) return;
      const h0 = cards[0].offsetHeight;
      let y = 0;
      cards.forEach((c, i) => {
        const lvl = Math.min(i, 2);
        const cs = 1 - lvl * 0.04;
        const h = c.offsetHeight;
        c.style.zIndex = String(40 - i);
        c.style.setProperty('--cy', `${(lvl * PEEK + h0 - h * cs).toFixed(1)}px`);
        c.style.setProperty('--cs', String(cs));
        c.style.setProperty('--co', String(i === 0 ? 1 : i === 1 ? 0.8 : i === 2 ? 0.6 : 0));
        c.style.setProperty('--cd', `${(cards.length - 1 - i) * 30}ms`);
        c.style.setProperty('--od', `${i * 40}ms`);
        c.style.setProperty('--oy', `${y}px`);
        y += h + 8;
      });
      stack.style.setProperty('--front-h', `${h0 + Math.min(cards.length - 1, 2) * PEEK}px`);
      const top = stack.getBoundingClientRect().top - deck.getBoundingClientRect().top;
      deck.style.setProperty('--veil-h', `${top + y - 8 + 14}px`);
    };
    layout();
    const ro = typeof ResizeObserver !== 'undefined' ? new ResizeObserver(layout) : null;
    for (const c of stack.children) ro?.observe(c);
    return () => ro?.disconnect();
  }, [items]);

  // The card that rises to the front after the front one leaves.
  useLayoutEffect(() => {
    const front = items[0]?.key;
    if (prevFront.current && front && front !== prevFront.current) {
      const el = stackRef.current?.firstElementChild as HTMLElement | null;
      if (el) {
        el.dataset.rising = '';
        setTimeout(() => delete el.dataset.rising, 500);
      }
    }
    prevFront.current = front;
  }, [items]);

  useLayoutEffect(() => {
    if (n < 2 && open) {
      setOpen(false);
      setPinned(false);
    }
  }, [n, open]);

  if (n === 0) return null;
  const front = items[0];
  const frontMark = front.form === 'await' ? 'awaiting' : front.form === 'error' ? 'failed' : markOf(front);
  const fold = () => {
    setPinned(false);
    setOpen(false);
  };

  return (
    <div
      ref={deckRef}
      className="evx-deck"
      data-testid="needs-you-deck"
      data-open={isOpen ? '' : undefined}
      data-no-gate={items.some((c) => c.form === 'await') ? undefined : ''}
      onMouseLeave={() => {
        onKin(null);
        if (pinned) return;
        clearTimeout(closeT.current);
        closeT.current = setTimeout(() => setOpen(false), 160);
      }}
      onKeyDown={(e) => {
        if (e.key === 'Escape') fold();
      }}
      role="region"
      aria-label="Needs you"
    >
      <div className="evx-veil" aria-hidden />
      <div className="evx-dedge" aria-hidden />
      <div className="evx-dshade" aria-hidden />
      <div className="evx-dt">pinned</div>
      <div className="evx-dm">
        <EvGlyph mark={frontMark} />
      </div>
      <div className="evx-dc">
        <div className="evx-dhead">
          <span className="evx-lbl">Needs you</span>
          <span className="evx-cnt">
            <Rolling value={n} />
          </span>
          <span className="evx-brief">{deckBrief(items)}</span>
          {n > 1 && (
            <button
              type="button"
              className="evx-showall"
              aria-expanded={isOpen}
              onClick={() => {
                const next = !pinned;
                setPinned(next);
                setOpen(next);
              }}
            >
              {isOpen ? 'Collapse' : `+${n - 1} more`}
              <ChevronDown size={12} aria-hidden />
            </button>
          )}
        </div>
        <div
          ref={stackRef}
          className="evx-stack"
          onMouseEnter={() => {
            if (!hoverable() || n < 2) return;
            clearTimeout(closeT.current);
            setOpen(true);
          }}
        >
          {items.map((card, i) => (
            <DeckCard
              key={card.key}
              card={card}
              index={i}
              arrived={arrived.has(card.key)}
              scope={scope}
              resolved={resolve(card)}
              gate={gate}
              crew={crewOf(card)}
              onKin={onKin}
              onSeen={(el) => {
                fold();
                onSeen(card, el);
              }}
              onAnswer={(el) => onAnswer(card, el)}
              onOpen={() => {
                fold();
                onOpen(card);
              }}
            />
          ))}
        </div>
      </div>
    </div>
  );
}

function DeckCard({
  card,
  index,
  arrived,
  scope,
  resolved,
  gate,
  crew,
  onKin,
  onSeen,
  onAnswer,
  onOpen,
}: {
  card: StreamCard;
  index: number;
  arrived: boolean;
  scope: RailScope;
  resolved: Resolved;
  gate?: EventGate;
  crew?: string;
  onKin: (thread: string | null) => void;
  onSeen: (el: HTMLElement | null) => void;
  onAnswer: (el: HTMLElement | null) => void;
  onOpen: () => void;
}) {
  const ref = useRef<HTMLDivElement | null>(null);
  const mark = card.form === 'await' ? 'awaiting' : card.form === 'error' ? 'failed' : markOf(card);
  const parts = ctxOf(card, scope, resolved);
  const style = { '--delay': `${index * 60}ms`, ...(crew ? { '--crew': crew } : {}) } as CSSProperties;
  // A gate answered here travels home once the page records the answer.
  const answeringGate: EventGate | undefined = gate && {
    state: gate.state,
    approve: (c) => {
      onAnswer(ref.current);
      gate.approve(c);
    },
    reject: (c) => {
      onAnswer(ref.current);
      gate.reject(c);
    },
  };
  return (
    <div
      ref={ref}
      className="evx-dcard"
      data-dkey={card.key}
      data-kind={card.form}
      data-front={index === 0 ? '' : undefined}
      data-beyond={index > 2 ? '' : undefined}
      data-arrived={arrived ? '' : undefined}
      style={style}
      onMouseOver={() => onKin(card.runId ?? card.crew ?? null)}
    >
      <div className="evx-glow" aria-hidden />
      <div className="evx-card evx-m" data-mark={mark}>
        <div className="evx-flash" aria-hidden />
        <div className="evx-head">
          <EvCap mark={mark} />
          <button type="button" className="evx-title" onClick={onOpen} title={`${card.title} — show it in the stream`}>
            {card.form === 'await' && card.stepId ? `gate ${card.stepId}` : card.title}
          </button>
          {card.form === 'finding' && card.fileRef && <span className="evx-loc">{card.fileRef}</span>}
          {card.form === 'await' && <span className="evx-loc">waiting</span>}
        </div>
        {card.form === 'await' && card.detail && <div className="evx-msg">{card.detail}</div>}
        {card.form === 'finding' && card.detail && (
          <div className="evx-why" data-one="">
            {card.detail}
          </div>
        )}
        {card.form === 'error' && card.detail && (
          <div className="evx-detail" data-one="">
            {card.detail.split('\n')[0]}
          </div>
        )}
        <div className="evx-dfoot">
          <span className="evx-ctx">
            <Ctx parts={parts} />
          </span>
          {card.form === 'await' ? (
            answeringGate ? (
              <GateButtons card={card} gate={answeringGate} compact />
            ) : (
              <span className="evx-acts">
                <button type="button" className="evx-gbtn" onClick={onOpen}>
                  <CornerDownRight size={12} aria-hidden />
                  Open
                </button>
              </span>
            )
          ) : (
            <span className="evx-acts">
              <button type="button" className="evx-gbtn" onClick={() => onSeen(ref.current)} aria-label={`Mark ${card.title} seen`}>
                <Check size={12} aria-hidden />
                Seen
              </button>
              <button type="button" className="evx-gbtn" onClick={onOpen}>
                <CornerDownRight size={12} aria-hidden />
                Open
              </button>
            </span>
          )}
        </div>
      </div>
    </div>
  );
}
