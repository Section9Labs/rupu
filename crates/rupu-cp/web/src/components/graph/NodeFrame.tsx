// The run graph's card parts (styles/graph-motion.css, `gx-*`):
//
//   NodeFrame    the card every node is drawn in: a raised panel with a soft
//                shadow and a hairline highlight. It owns the node's motion:
//                its entrance slot, its state (a running card floats a little
//                higher on a breathing under-glow with a progress line along
//                its foot; a card that waits on the operator glows amber; a
//                failed one holds a red border), the one-shot cue a state
//                change plays (a green bloom under a card that lands, a red
//                flash on one that fails), hover lift and the selection ring.
//   NodeHead     kind tile · name · extras · status capsule.
//   NodeFoot     the kind label (and anything the node puts on its right).
//   StatusCapsule / StatusGlyph
//                the status: one glyph that morphs between states (a spinning
//                arc that closes into a ring and grows a check, an X that draws
//                itself, a dashed ring for pending, a breathing dot for "needs
//                you") beside its word, so every state reads without color.

import type { CSSProperties, ReactNode } from 'react';
import { Handle, Position } from '@xyflow/react';
import type { StepState } from '../../lib/runGraphModel';
import { useStateCue } from '../../lib/graph/runMotion';
import { colorChannels } from '../../lib/useThemeColors';
import { cn } from '../../lib/cn';
import { runKindAccent, runKindIcon, runKindLabel, type RunKind } from './kindBridge';
import { MessageCount } from './MessageCount';

export function NodeFrame({
  state,
  kind,
  slot,
  width,
  minHeight,
  testId = 'rg-node',
  live = state === 'running',
  lit,
  className,
  style,
  children,
}: {
  state: StepState;
  kind: RunKind;
  /** Entrance slot (see `useEntranceSlots`); 0 when unknown. */
  slot?: number;
  width: number;
  minHeight: number;
  testId?: string;
  /** Show the progress line along the foot (a running card). */
  live?: boolean;
  /** An end of the selected (or hovered) message: ringed like a selection,
   *  lighter. */
  lit?: boolean;
  className?: string;
  style?: CSSProperties;
  children: ReactNode;
}) {
  const { cue } = useStateCue(state);
  return (
    <div
      data-state={state}
      data-cue={cue ?? undefined}
      data-lit={lit || undefined}
      className="gx-node gx-s"
      style={{ ['--i' as string]: slot ?? 0, ['--k' as string]: colorChannels(runKindAccent(kind)), width }}
    >
      <span className="gx-glow" aria-hidden />
      <span className="gx-bloom" aria-hidden />
      <span className="gx-lift" aria-hidden />
      <div data-testid={testId} className={cn('gx-card', className)} style={{ minHeight, ...style }}>
        <span className="gx-flash" aria-hidden />
        {children}
        {live && <RunLine />}
      </div>
      <span className="gx-sel" aria-hidden />
      <Handle type="target" position={Position.Left} className="gx-port" />
      <Handle type="source" position={Position.Right} className="gx-port" />
    </div>
  );
}

/** The indeterminate progress line a working card (or row) carries. */
export function RunLine() {
  return (
    <span className="gx-runline" aria-hidden>
      <i />
    </span>
  );
}

/** The kind's icon in a tile tinted with the kind's color. */
export function KindTile({ kind }: { kind: RunKind }) {
  const Icon = runKindIcon(kind);
  return (
    <span data-testid="rg-kindpill" className="gx-tile" title={runKindLabel(kind)}>
      <Icon size={12} strokeWidth={1.9} aria-hidden />
    </span>
  );
}

export function NodeHead({
  kind,
  name,
  state,
  mono,
  messages,
  children,
}: {
  kind: RunKind;
  name: string;
  state: StepState;
  /** Messages the card sent or received (a badge; none when 0). */
  messages?: number;
  /** Set the name as a code identifier (an action's tool name). */
  mono?: boolean;
  /** Extras between the name and the capsule (counts, warnings). */
  children?: ReactNode;
}) {
  return (
    <div className="gx-head">
      <KindTile kind={kind} />
      <span className={cn('gx-name', mono && 'font-mono')} title={name}>
        {name}
      </span>
      {children}
      <MessageCount n={messages} />
      <StatusCapsule state={state} />
    </div>
  );
}

export function NodeFoot({ kind, children }: { kind: RunKind; children?: ReactNode }) {
  return (
    <div className="gx-foot">
      <span className="gx-kind">{runKindLabel(kind)}</span>
      {children}
    </div>
  );
}

/** The word a status capsule shows. "needs you" for a gate that waits on the
 *  operator: it names what to do, not the machine state. */
export const STATE_WORD: Record<StepState, string> = {
  running: 'running',
  done: 'done',
  failed: 'failed',
  awaiting_approval: 'needs you',
  paused: 'paused',
  pending: 'pending',
  skipped: 'skipped',
};

/** States whose word earns a place on the card: the ones that need a look.
 *  The calm ones (done, pending, skipped) show their glyph alone, so a long
 *  step name keeps its room; the word stays for screen readers and hover. */
const LOUD: ReadonlySet<StepState> = new Set(['running', 'failed', 'awaiting_approval', 'paused']);

export function StatusCapsule({ state, small }: { state: StepState; small?: boolean }) {
  const loud = LOUD.has(state);
  return (
    <span
      data-state={state}
      className={cn('gx-cap gx-s', small && 'gx-cap-sm', !loud && 'gx-cap-quiet')}
      title={loud ? undefined : STATE_WORD[state]}
    >
      <StatusGlyph state={state} />
      <span key={state} className={loud ? 'gx-word' : 'sr-only'}>
        {STATE_WORD[state]}
      </span>
    </span>
  );
}

/** One glyph for every state; CSS morphs between them (see the capsule note
 *  above). The spinning arc never stops its rotation, only pauses it: a closed
 *  ring looks the same at any angle, so running → done never snaps back. */
export function StatusGlyph({ state }: { state: StepState }) {
  return (
    <svg data-state={state} className="gx-glyph gx-s" viewBox="0 0 16 16" aria-hidden>
      <g className="gx-spin">
        <circle className="gx-g-ring" cx="8" cy="8" r="5.5" />
      </g>
      <circle className="gx-g-dash" cx="8" cy="8" r="5.5" />
      <path className="gx-g-check" d="M5 8.2l2.1 2.1 4.2-4.2" />
      <path className="gx-g-x1" d="M5.6 5.6l4.8 4.8" />
      <path className="gx-g-x2" d="M10.4 5.6l-4.8 4.8" />
      <circle className="gx-g-dot" cx="8" cy="8" r="1.9" />
      <path className="gx-g-pause" d="M6.6 6v4M9.4 6v4" />
      <path className="gx-g-skip" d="M5.2 8h5.6" />
    </svg>
  );
}
