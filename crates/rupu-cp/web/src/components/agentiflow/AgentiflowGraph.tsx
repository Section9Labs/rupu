// AgentiflowGraph — the branching flow of a goal-directed fleet, drawn the way
// rupu's own git-graph view draws a workflow: the LEAD is the spine (engagement
// started → round 0 → round 1 → … → stopped), and every unit the lead dispatches
// in a round FANS OUT as a branch off that round's node and MERGES back into the
// next one. It is the live-events timeline and the run graph fused into one, and
// it grows — taller per round, wider per branch — as the run unfolds.
//
// Recursion: a dispatched WORKFLOW unit is itself a full workflow run, so it is
// not a leaf. Expanding it unfolds its own flow DOWNWARD — the workflow's steps,
// indented one level off the unit, hanging from a crew-tinted sub-spine — so the
// path keeps flowing into the workflow instead of dead-ending at a single line.
// Sub-flows start collapsed and lazy-load `GET /api/runs/:id/graph` on expand,
// so a 160-unit engagement never drags in 160 sub-graphs it isn't showing.
//
// Pure over `GET /api/agentiflows/:id` (events + units + lead_transcripts) plus
// the per-unit sub-graph cache: the geometry is computed once per render, so a
// `running` run's poll just redraws a taller spine. Colors come from the real
// codename palette — the spine is tinted by the run's own crew (the lead), each
// branch and each nested sub-spine by its unit's crew.

import { useCallback, useMemo, useState } from 'react';
import { Link } from 'react-router-dom';
import { ChevronRight, ChevronDown } from 'lucide-react';
import { AgentName } from '../codename/AgentName';
import { StatusPill } from '../StatusPill';
import { Badge } from '../ui/Badge';
import { useThemeMode } from '../codename/useThemeMode';
import { parseCodename, crewTint } from '../../lib/codename';
import { unitPillStatus, budgetStateTone, stopReasonTone, STOP_TONE_TEXT, formatSpendUsd, formatSpendTokens, STOP_TONE_BADGE } from '../../lib/agentiflow';
import { cn } from '../../lib/cn';
import { relativeTime, absoluteTime } from '../../lib/time';
import { api } from '../../lib/api';
import type { AgentiflowDetail, AgentiflowUnit, AgentiflowEvent, RunGraphResponse, StepNodeDto } from '../../lib/api';

// Gutter geometry (px). The spine lives at SPINE_X, each round's branch lane at
// BRANCH_X, and row content starts at CARD_X — everything left of it is drawn in
// the SVG gutter.
const SPINE_X = 22;
const BRANCH_X = 46;
const CARD_X = 70;
const HEADER_H = 50; // a start / round / stop spine row
const ROW_H = 46; // a unit branch row
const PULSE_H = 34; // the "running…" tail
const NODE_R = 6;
// Nested sub-flow geometry (a workflow unit's own steps, one level deeper).
const SUB_INDENT = 20; // how far the sub-spine sits in from the unit card's left
const SUB_ROW_H = 30; // a nested step row
const SUB_TOP_GAP = 4; // breathing room between the unit card and its sub-flow

type Palette = ReturnType<typeof palette>;
function palette(mode: 'light' | 'dark') {
  return mode === 'dark'
    ? { lane: '#3a3a40', mute: '#71717a', err: '#f87171', ok: '#4ade80', warn: '#fbbf24', brand: '#a78bfa' }
    : { lane: '#d4d4d8', mute: '#a1a1aa', err: '#dc2626', ok: '#16a34a', warn: '#d97706', brand: '#7c3aed' };
}

/** A nested step's live state, folded from the sub-run's step_results. */
type NestState = 'done' | 'failed' | 'running' | 'awaiting' | 'pending' | 'skipped';

const NEST_VIS: Record<NestState, { glyph: string; label: string; key: keyof Omit<Palette, 'lane'> }> = {
  done: { glyph: '✓', label: 'done', key: 'ok' },
  failed: { glyph: '✕', label: 'failed', key: 'err' },
  running: { glyph: '⟳', label: 'running', key: 'brand' },
  awaiting: { glyph: '⏸', label: 'awaiting', key: 'warn' },
  pending: { glyph: '•', label: 'pending', key: 'mute' },
  skipped: { glyph: '⤼', label: 'skipped', key: 'mute' },
};

/** One nested step row inside an expanded workflow unit. */
interface SubStepRow {
  id: string;
  kind: StepNodeDto['kind'];
  agent?: string | null;
  codename?: string;
  codenameDerived?: boolean;
  state: NestState;
}

/** The sub-flow that hangs off an expanded workflow unit. */
interface SubFlow {
  state: 'loading' | 'error' | 'loaded';
  error?: string;
  tint: string; // the unit's crew tint — the sub-spine's colour
  steps: SubStepRow[]; // empty for loading / error
}

/** Cache entry for a workflow unit's fetched run graph. */
interface SubCache {
  state: 'loading' | 'error' | 'loaded';
  graph?: RunGraphResponse;
  error?: string;
}

interface UnitNode {
  unit: AgentiflowUnit;
  cy: number; // branch-row (card) center y
  tint: string; // crew tint (or mute)
  dotFill: string | null; // null = hollow (pending)
  dotRing: string | null; // colored ring (failed)
  pulse: boolean; // running
  blockTop: number; // top of the unit's block (card + any sub-flow)
  blockHeight: number; // ROW_H, plus the sub-flow when expanded
  sub?: SubFlow; // present only for an expanded workflow unit
}

interface SpineNode {
  kind: 'start' | 'round' | 'stop' | 'pulse';
  key: string;
  top: number;
  height: number;
  nodeY: number;
  round?: number;
  ev?: AgentiflowEvent;
  transcriptPath?: string;
  units: UnitNode[];
  /** A round whose units are hidden (long-flow collapse). */
  collapsed?: boolean;
  /** Total units in the round, even when collapsed (units[] is then empty). */
  unitCount?: number;
}

function str(v: unknown): string | undefined {
  return typeof v === 'string' && v !== '' ? v : undefined;
}
function num(v: unknown): number | undefined {
  return typeof v === 'number' && Number.isFinite(v) ? v : undefined;
}

/** A smooth S-curve from the spine at `y0` out to the branch lane at `y1`. */
function spur(x0: number, y0: number, x1: number, y1: number): string {
  const mx = (x0 + x1) / 2;
  return `M ${x0} ${y0} C ${mx} ${y0}, ${mx} ${y1}, ${x1} ${y1}`;
}

/** A workflow unit's step state, folded from the sub-run's step_results. */
function deriveStepState(stepId: string, results: Map<string, { success?: boolean; skipped?: boolean }>): NestState {
  const r = results.get(stepId);
  if (!r) return 'pending'; // no terminal record for this step yet
  if (r.skipped) return 'skipped';
  if (r.success === true) return 'done';
  if (r.success === false) return 'failed';
  return 'running'; // has a record but no verdict → in flight
}

/** Build the sub-flow descriptor from a cache entry (or a loading/error stub). */
function toSubFlow(cache: SubCache | undefined, tint: string): SubFlow {
  if (!cache || cache.state === 'loading') return { state: 'loading', tint, steps: [] };
  if (cache.state === 'error') return { state: 'error', error: cache.error, tint, steps: [] };
  const g = cache.graph!;
  const results = new Map<string, { success?: boolean; skipped?: boolean }>();
  // A step's own codename rides on its step_result; agent/provider/model (and a
  // codename for records that predate it) come from the folded step_identities.
  const cnByStep = new Map<string, { codename: string; derived?: boolean }>();
  for (const r of g.step_results ?? []) {
    results.set(r.step_id, { success: r.success, skipped: r.skipped });
    if (r.codename) cnByStep.set(r.step_id, { codename: r.codename, derived: r.codename_derived });
  }
  const steps: SubStepRow[] = (g.workflow?.steps ?? []).map((s) => {
    const sid = g.step_identities?.[s.id];
    const cn = cnByStep.get(s.id) ?? (sid?.codename ? { codename: sid.codename, derived: sid.codename_derived } : undefined);
    return {
      id: s.id,
      kind: s.kind,
      agent: sid?.agent ?? s.agent ?? undefined,
      codename: cn?.codename,
      codenameDerived: cn?.derived,
      state: deriveStepState(s.id, results),
    };
  });
  return { state: 'loaded', tint, steps };
}

export default function AgentiflowGraph({ detail }: { detail: AgentiflowDetail }) {
  const mode = useThemeMode();
  const pal = palette(mode);
  const { record } = detail;
  const leadCrew = parseCodename(record.codename).crew;
  const leadTint = crewTint(leadCrew, mode) ?? pal.brand;

  // Per-round collapse override (round → explicit expanded state). Without an
  // entry a round defaults to collapsed unless it is the last / live one, so a
  // long engagement stays compact and only the active round is open.
  const [expandedOverride, setExpandedOverride] = useState<Record<number, boolean>>({});
  const toggleRound = (round: number, currentlyExpanded: boolean) =>
    setExpandedOverride((o) => ({ ...o, [round]: !currentlyExpanded }));

  // Per-workflow-unit recursion. A unit is expanded only after the operator
  // opens it; opening lazy-fetches its run graph into `subCache` (keyed by the
  // unit's run id, so it survives the detail poll and is fetched once).
  const [expandedUnits, setExpandedUnits] = useState<Record<string, boolean>>({});
  const [subCache, setSubCache] = useState<Record<string, SubCache>>({});

  const loadSub = useCallback((unitId: string) => {
    setSubCache((c) => (c[unitId]?.state === 'loaded' ? c : { ...c, [unitId]: { state: 'loading' } }));
    api.getRunGraph(unitId).then(
      (graph) => setSubCache((c) => ({ ...c, [unitId]: { state: 'loaded', graph } })),
      (e: unknown) => setSubCache((c) => ({ ...c, [unitId]: { state: 'error', error: e instanceof Error ? e.message : String(e) } })),
    );
  }, []);

  const toggleUnit = useCallback(
    (unitId: string) => {
      setExpandedUnits((prev) => {
        const next = !prev[unitId];
        if (next) loadSub(unitId);
        return { ...prev, [unitId]: next };
      });
    },
    [loadSub],
  );

  const { nodes, totalH } = useMemo(
    () => build(detail, mode, pal, expandedOverride, expandedUnits, subCache),
    // detail identity + mode + the collapse/expand state drive the geometry.
    [detail, mode, expandedOverride, expandedUnits, subCache], // eslint-disable-line react-hooks/exhaustive-deps
  );

  const lastY = nodes.length ? nodes[nodes.length - 1].nodeY : 0;
  const firstY = nodes.length ? nodes[0].nodeY : 0;

  return (
    <div className="relative overflow-x-auto" style={{ minHeight: totalH }}>
      <div className="relative" style={{ height: totalH }}>
        <svg
          className="pointer-events-none absolute left-0 top-0"
          width={CARD_X}
          height={totalH}
          aria-hidden
        >
          {/* The lead spine — one continuous line through every spine node. */}
          {nodes.length > 1 && (
            <line
              x1={SPINE_X}
              y1={firstY}
              x2={SPINE_X}
              y2={lastY}
              stroke={leadTint}
              strokeOpacity={0.5}
              strokeWidth={2}
            />
          )}

          {/* Per round: a branch lane that fans out of the round node and merges
              into the next spine node, plus a tick + dot per unit. */}
          {nodes.map((n, i) => {
            if (n.kind !== 'round' || n.units.length === 0) return null;
            const next = nodes[i + 1];
            const firstU = n.units[0];
            const lastU = n.units[n.units.length - 1];
            return (
              <g key={`branch-${n.key}`}>
                {/* fan-out from the round node to the lane top */}
                <path d={spur(SPINE_X, n.nodeY, BRANCH_X, firstU.cy)} fill="none" stroke={pal.lane} strokeWidth={1.5} />
                {/* the lane itself */}
                {n.units.length > 1 && (
                  <line x1={BRANCH_X} y1={firstU.cy} x2={BRANCH_X} y2={lastU.cy} stroke={pal.lane} strokeWidth={1.5} />
                )}
                {/* merge back into the next spine node */}
                {next && (
                  <path d={spur(BRANCH_X, lastU.cy, SPINE_X, next.nodeY)} fill="none" stroke={pal.lane} strokeWidth={1.5} strokeOpacity={0.7} />
                )}
                {/* per-unit tick + dot */}
                {n.units.map((u) => (
                  <g key={u.unit.unit_id}>
                    <line x1={BRANCH_X} y1={u.cy} x2={CARD_X - 6} y2={u.cy} stroke={u.tint} strokeOpacity={0.6} strokeWidth={1.5} />
                    <circle
                      cx={BRANCH_X}
                      cy={u.cy}
                      r={4}
                      fill={u.dotFill ?? (mode === 'dark' ? '#141416' : '#ffffff')}
                      stroke={u.dotRing ?? u.tint}
                      strokeWidth={1.6}
                      className={u.pulse ? 'animate-pulse' : undefined}
                    />
                  </g>
                ))}
              </g>
            );
          })}

          {/* Spine node glyphs */}
          {nodes.map((n) => {
            if (n.kind === 'pulse') {
              return <circle key={n.key} cx={SPINE_X} cy={n.nodeY} r={5} fill={leadTint} className="animate-pulse" />;
            }
            const fill =
              n.kind === 'stop'
                ? record.stop_reason
                  ? stopTone(record.stop_reason, pal)
                  : pal.mute
                : leadTint;
            return (
              <g key={n.key}>
                <circle cx={SPINE_X} cy={n.nodeY} r={NODE_R} fill={fill} />
                {n.kind === 'start' && (
                  <circle cx={SPINE_X} cy={n.nodeY} r={NODE_R + 3} fill="none" stroke={fill} strokeOpacity={0.35} strokeWidth={1.5} />
                )}
              </g>
            );
          })}
        </svg>

        {/* Row content — aligned to the SVG node y's. */}
        {nodes.map((n) => (
          <div key={`row-${n.key}`} className="absolute" style={{ top: n.top, left: CARD_X, right: 0, height: n.kind === 'round' ? HEADER_H : n.height }}>
            <RowHead node={n} detail={detail} leadCrew={leadCrew} onToggle={toggleRound} />
          </div>
        ))}
        {nodes.flatMap((n) =>
          n.units.map((u) => (
            <div
              key={`unit-${u.unit.unit_id}`}
              className="absolute"
              style={{ top: u.blockTop, left: CARD_X, right: 0, height: u.blockHeight }}
            >
              <div className="flex items-center" style={{ height: ROW_H }}>
                <UnitCard
                  unit={u.unit}
                  expandable={u.unit.kind === 'workflow'}
                  expanded={!!expandedUnits[u.unit.unit_id]}
                  onToggle={() => toggleUnit(u.unit.unit_id)}
                />
              </div>
              {u.sub && <SubFlowRows sub={u.sub} pal={pal} />}
            </div>
          )),
        )}
      </div>
    </div>
  );
}

function stopTone(reason: string, pal: Palette): string {
  const t = stopReasonTone(reason);
  return t === 'ok' ? pal.ok : t === 'err' || t === 'warn' ? pal.err : pal.mute;
}

// ---------------------------------------------------------------------------
// Geometry
// ---------------------------------------------------------------------------

function build(
  detail: AgentiflowDetail,
  mode: 'light' | 'dark',
  pal: Palette,
  override: Record<number, boolean>,
  expandedUnits: Record<string, boolean>,
  subCache: Record<string, SubCache>,
): { nodes: SpineNode[]; totalH: number } {
  const { record, events, units, lead_transcripts } = detail;
  const running = record.status === 'running';

  const startEv = events.find((e) => e.kind === 'run_started');
  const stopEv = events.find((e) => e.kind === 'run_stopped');
  const roundEvs = events
    .filter((e) => e.kind === 'round')
    .map((e) => ({ ev: e, round: num(e.round) ?? 0, ts: Date.parse(str(e.ts) ?? '') }))
    .sort((a, b) => a.round - b.round);
  const roundEvByIdx = new Map(roundEvs.map((r) => [r.round, r]));
  const txByRound = new Map(lead_transcripts.map((t) => [t.round, t.path]));

  // Which round each unit ran in: the first round whose end-ts is at or after
  // the unit's start (it ended no earlier than the unit began). Units with no
  // start, or started after the last recorded round, fall into the live round.
  const inProgressIdx = running ? roundEvs.length : null;
  const bucketRound = (u: AgentiflowUnit): number => {
    const t = u.started_at ? Date.parse(u.started_at) : NaN;
    if (!Number.isNaN(t)) {
      const hit = roundEvs.find((r) => !Number.isNaN(r.ts) && r.ts >= t);
      if (hit) return hit.round;
    }
    return inProgressIdx ?? (roundEvs.length ? roundEvs[roundEvs.length - 1].round : 0);
  };
  const unitsByRound = new Map<number, AgentiflowUnit[]>();
  for (const u of units) {
    const r = bucketRound(u);
    (unitsByRound.get(r) ?? unitsByRound.set(r, []).get(r)!).push(u);
  }

  const maxRound = Math.max(
    roundEvs.length ? roundEvs[roundEvs.length - 1].round : -1,
    inProgressIdx ?? -1,
    ...[...unitsByRound.keys()],
    -1,
  );

  // How tall a sub-flow is: one row per step (min one, so loading / error /
  // empty still reserve a line), plus the gap above it.
  const subHeight = (sub: SubFlow): number => {
    const rows = sub.state === 'loaded' ? Math.max(sub.steps.length, 1) : 1;
    return SUB_TOP_GAP + rows * SUB_ROW_H;
  };

  const toUnitNode = (u: AgentiflowUnit, blockTop: number): UnitNode => {
    const tint = crewTint(parseCodename(u.codename).crew, mode) ?? pal.mute;
    const st = unitPillStatus(u);
    const expanded = u.kind === 'workflow' && !!expandedUnits[u.unit_id];
    const sub = expanded ? toSubFlow(subCache[u.unit_id], tint) : undefined;
    return {
      unit: u,
      cy: blockTop + ROW_H / 2,
      tint,
      dotFill: st === 'failed' ? null : st === 'pending' ? null : tint,
      dotRing: st === 'failed' ? pal.err : null,
      pulse: st === 'running',
      blockTop,
      blockHeight: ROW_H + (sub ? subHeight(sub) : 0),
      sub,
    };
  };

  const nodes: SpineNode[] = [];
  let y = 0;
  const pushHead = (node: Omit<SpineNode, 'top' | 'nodeY' | 'height' | 'units'> & { units?: UnitNode[]; height?: number }) => {
    const height = node.height ?? HEADER_H;
    const top = y;
    const nodeY = top + HEADER_H / 2;
    nodes.push({ ...node, top, height, nodeY, units: node.units ?? [] });
    y += height;
  };

  if (startEv) pushHead({ kind: 'start', key: 'start', ev: startEv });

  for (let r = 0; r <= maxRound; r++) {
    const us = unitsByRound.get(r) ?? [];
    // Default: only the last / live round is open; earlier rounds collapse
    // unless the operator expanded one. A round with no units never "collapses".
    const expanded = us.length === 0 ? true : r in override ? override[r] : r === maxRound;
    const top = y;
    // Lay the round's units out one block under another — a block is the unit
    // card plus, for an expanded workflow unit, its nested sub-flow.
    let by = top + HEADER_H;
    const unitNodes: UnitNode[] = [];
    if (expanded) {
      for (const u of us) {
        const node = toUnitNode(u, by);
        unitNodes.push(node);
        by += node.blockHeight;
      }
    }
    const bodyH = by - (top + HEADER_H);
    nodes.push({
      kind: 'round',
      key: `round-${r}`,
      round: r,
      ev: roundEvByIdx.get(r)?.ev,
      transcriptPath: txByRound.get(r),
      top,
      nodeY: top + HEADER_H / 2,
      height: HEADER_H + bodyH,
      units: unitNodes,
      collapsed: us.length > 0 && !expanded,
      unitCount: us.length,
    });
    y += HEADER_H + bodyH;
  }

  if (stopEv) pushHead({ kind: 'stop', key: 'stop', ev: stopEv });
  else if (running) pushHead({ kind: 'pulse', key: 'pulse', height: PULSE_H });

  return { nodes, totalH: y };
}

// ---------------------------------------------------------------------------
// Row content
// ---------------------------------------------------------------------------

function RowHead({
  node,
  detail,
  leadCrew,
  onToggle,
}: {
  node: SpineNode;
  detail: AgentiflowDetail;
  leadCrew: string;
  onToggle: (round: number, currentlyExpanded: boolean) => void;
}) {
  if (node.kind === 'start') {
    const profiles = detail.record.engagement_profiles;
    return (
      <div className="flex h-[50px] flex-col justify-center">
        <div className="flex flex-wrap items-center gap-2">
          <span className="text-sm font-semibold text-ink">Engagement started</span>
          {profiles.map((p) => (
            <Badge key={p} tone="violet">
              {p}
            </Badge>
          ))}
        </div>
        <div className="text-meta text-ink-mute" title={absoluteTime(detail.record.started_at)}>
          lead <span className="font-mono">{leadCrew}</span> · {relativeTime(detail.record.started_at)}
        </div>
      </div>
    );
  }

  if (node.kind === 'stop') {
    const reason = detail.record.stop_reason;
    return (
      <div className="flex h-[50px] flex-col justify-center">
        <div className="flex flex-wrap items-center gap-2">
          <span className="text-sm font-semibold text-ink">Stopped</span>
          {reason && <span className={cn('font-mono text-note', STOP_TONE_TEXT[stopReasonTone(reason)])}>{reason}</span>}
        </div>
        {detail.record.ended_at && (
          <div className="text-meta text-ink-mute" title={absoluteTime(detail.record.ended_at)}>
            {relativeTime(detail.record.ended_at)}
          </div>
        )}
      </div>
    );
  }

  if (node.kind === 'pulse') {
    return (
      <div className="flex h-[34px] items-center">
        <span className="text-note text-ink-dim">
          round {node.round ?? ''} running<span className="animate-pulse">…</span>
        </span>
      </div>
    );
  }

  // round
  const ev = node.ev;
  const budget = str(ev?.budget);
  const met = num(ev?.goals_met);
  const total = num(ev?.goals_total);
  const usd = num(ev?.spent_usd);
  const tokens = num(ev?.spent_tokens);
  const converge = ev?.converge === true;
  const live = !ev; // no round event yet → this round is in progress
  const unitCount = node.unitCount ?? 0;
  const expanded = !node.collapsed;
  return (
    <div className="flex h-[50px] flex-col justify-center">
      <div className="flex flex-wrap items-center gap-x-2.5 gap-y-1">
        {unitCount > 0 && (
          <button
            type="button"
            onClick={() => onToggle(node.round!, expanded)}
            className="-ml-1 inline-flex items-center rounded text-ink-mute hover:text-ink"
            aria-label={expanded ? `Collapse round ${node.round}` : `Expand round ${node.round}`}
            title={expanded ? 'Collapse round' : 'Expand round'}
          >
            {expanded ? <ChevronDown size={14} /> : <ChevronRight size={14} />}
          </button>
        )}
        <span className="text-sm font-semibold text-ink">round {node.round}</span>
        {live ? (
          <span className="text-meta text-ink-dim">
            in progress<span className="animate-pulse">…</span>
          </span>
        ) : (
          <>
            {met != null && total != null && (
              <span className="text-note tabular-nums text-ink-dim">
                goals {met}/{total}
              </span>
            )}
            {budget && <Badge tone={STOP_TONE_BADGE[budgetStateTone(budget)]}>{budget}</Badge>}
            {converge && <span className="text-note text-ok">converging</span>}
            {(usd != null || tokens != null) && (
              <span className="text-meta tabular-nums text-ink-mute">
                {formatSpendUsd(usd)}
                {tokens != null && ` · ${formatSpendTokens(tokens)}`}
              </span>
            )}
          </>
        )}
        {unitCount > 0 && (
          <button
            type="button"
            onClick={() => onToggle(node.round!, expanded)}
            className="text-meta text-ink-mute hover:text-ink"
          >
            · {unitCount} {unitCount === 1 ? 'unit' : 'units'}
            {node.collapsed && ' (hidden)'}
          </button>
        )}
        {node.transcriptPath && (
          <Link
            to={`/transcript?path=${encodeURIComponent(node.transcriptPath)}&live=${live ? 1 : 0}`}
            className="text-meta font-medium text-brand-600 hover:text-brand-700 hover:underline"
          >
            lead transcript
          </Link>
        )}
      </div>
    </div>
  );
}

function UnitCard({
  unit,
  expandable,
  expanded,
  onToggle,
}: {
  unit: AgentiflowUnit;
  expandable: boolean;
  expanded: boolean;
  onToggle: () => void;
}) {
  return (
    <div className="flex min-w-0 flex-1 items-center gap-x-2.5 gap-y-1 rounded-lg border border-border bg-surface/60 px-2.5 py-1.5">
      {expandable && (
        <button
          type="button"
          onClick={onToggle}
          className="-ml-1 inline-flex shrink-0 items-center rounded text-ink-mute hover:text-ink"
          aria-label={expanded ? `Collapse ${unit.codename} flow` : `Expand ${unit.codename} flow`}
          title={expanded ? 'Collapse flow' : 'Expand flow'}
        >
          {expanded ? <ChevronDown size={14} /> : <ChevronRight size={14} />}
        </button>
      )}
      <AgentName codename={unit.codename} agent={unit.agent ?? undefined} derived={unit.codename_derived} showCrew />
      <Badge tone="neutral" ring>
        {unit.kind}
      </Badge>
      <StatusPill status={unitPillStatus(unit)} />
      {unit.participant && <span className="truncate font-mono text-meta text-ink-mute">{unit.participant}</span>}
      <span className="ml-auto flex shrink-0 items-center gap-2.5 text-meta text-ink-mute">
        {unit.started_at && <span title={absoluteTime(unit.started_at)}>{relativeTime(unit.started_at)}</span>}
        {unit.kind === 'workflow' ? (
          // A dispatched workflow IS a full workflow run. Expand it inline to
          // see its flow here, or open the full DAG on its own run page.
          <Link
            to={`/runs/${encodeURIComponent(unit.unit_id)}`}
            className="font-medium text-brand-600 hover:text-brand-700 hover:underline"
            title="Open this workflow's run graph"
          >
            open flow →
          </Link>
        ) : (
          unit.transcript_path && (
            <Link
              to={`/transcript?path=${encodeURIComponent(unit.transcript_path)}&live=0`}
              className="font-medium text-brand-600 hover:text-brand-700 hover:underline"
            >
              transcript
            </Link>
          )
        )}
      </span>
    </div>
  );
}

/** The nested sub-flow: a crew-tinted sub-spine with one row per workflow step,
 *  indented a level off the unit card. Lives entirely in HTML (the SVG gutter
 *  only draws the lead spine + round branches) so the main graph is untouched. */
function SubFlowRows({ sub, pal }: { sub: SubFlow; pal: Palette }) {
  const rows = sub.state === 'loaded' ? Math.max(sub.steps.length, 1) : 1;
  const height = rows * SUB_ROW_H;
  return (
    <div className="relative" style={{ marginTop: SUB_TOP_GAP, marginLeft: SUB_INDENT, height }}>
      {/* the sub-spine */}
      <div className="absolute top-0 bottom-0" style={{ left: 0, width: 2, background: sub.tint, opacity: 0.55 }} />
      {sub.state === 'loading' && <SubNote pal={pal} text="loading flow…" tint={sub.tint} />}
      {sub.state === 'error' && <SubNote pal={pal} text="couldn't load this flow" tint={pal.err} tone={pal.err} />}
      {sub.state === 'loaded' && sub.steps.length === 0 && <SubNote pal={pal} text="no steps recorded" tint={sub.tint} />}
      {sub.state === 'loaded' &&
        sub.steps.map((s, i) => <SubStep key={`${s.id}-${i}`} step={s} top={i * SUB_ROW_H} pal={pal} />)}
    </div>
  );
}

function SubNote({ text, tint, tone, pal }: { text: string; tint: string; tone?: string; pal: Palette }) {
  return (
    <div className="absolute flex items-center" style={{ top: 0, left: 0, right: 0, height: SUB_ROW_H, paddingLeft: 16 }}>
      <span className="absolute" style={{ left: -4, top: SUB_ROW_H / 2 - 3.5, width: 7, height: 7, borderRadius: 9999, background: tint, opacity: 0.6 }} />
      <span className="text-meta" style={{ color: tone ?? pal.mute }}>
        {text}
      </span>
    </div>
  );
}

function SubStep({ step, top, pal }: { step: SubStepRow; top: number; pal: Palette }) {
  const vis = NEST_VIS[step.state];
  const color = pal[vis.key];
  return (
    <div className="absolute flex items-center gap-x-2 gap-y-1" style={{ top, left: 0, right: 0, height: SUB_ROW_H, paddingLeft: 16 }}>
      {/* node on the sub-spine */}
      <span
        className={cn('absolute', step.state === 'running' && 'animate-pulse')}
        style={{ left: -4, top: SUB_ROW_H / 2 - 3.5, width: 7, height: 7, borderRadius: 9999, background: color }}
        aria-hidden
      />
      <span className="truncate text-note font-medium text-ink">{step.id}</span>
      {step.kind !== 'step' && (
        <Badge tone="neutral" ring>
          {step.kind}
        </Badge>
      )}
      {step.codename ? (
        <AgentName codename={step.codename} agent={step.agent ?? undefined} derived={step.codenameDerived} />
      ) : (
        step.agent && <span className="truncate font-mono text-meta text-ink-mute">{step.agent}</span>
      )}
      <span className="ml-auto flex shrink-0 items-center gap-1 text-meta" style={{ color }}>
        <span aria-hidden>{vis.glyph}</span>
        {vis.label}
      </span>
    </div>
  );
}
