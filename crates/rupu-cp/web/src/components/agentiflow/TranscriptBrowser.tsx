// TranscriptBrowser — the agentiflow Transcript tab as a two-pane browser, the
// way a workflow run exposes its transcripts: a searchable list of EVERY
// transcript on the left (grouped by round — the lead's transcript for the round
// plus each unit the lead dispatched, with a dispatched workflow's own step
// transcripts nested under it), and the selected transcript rendered on the
// right (the shared TranscriptPanel, tailing live for the running lead round).
//
// The old tab showed only the last lead round; you had to click units one by
// one from the Flow tab to reach them. This lists them all.
//
// Scale: a long engagement has hundreds of transcripts (one lead per round +
// every unit), so rounds collapse by default (only the last / live round open),
// a workflow unit's sub-transcripts lazy-load `GET /api/runs/:id/graph` on
// expand, and a filter box narrows the list.

import { useCallback, useMemo, useState } from 'react';
import { ChevronRight, ChevronDown, Search, Radio } from 'lucide-react';
import TranscriptPanel from '../TranscriptPanel';
import { AgentName } from '../codename/AgentName';
import { useThemeMode } from '../codename/useThemeMode';
import { parseCodename, crewTint } from '../../lib/codename';
import { unitPillStatus } from '../../lib/agentiflow';
import { cn } from '../../lib/cn';
import { api } from '../../lib/api';
import type { AgentiflowDetail, AgentiflowUnit, RunGraphResponse, StepNodeDto } from '../../lib/api';

type Mode = 'light' | 'dark';

/** The transcript the right pane is showing. */
interface Selection {
  path: string;
  live: boolean;
  title: string;
  sub: string;
}

interface SubCache {
  state: 'loading' | 'error' | 'loaded';
  graph?: RunGraphResponse;
  error?: string;
}

function str(v: unknown): string | undefined {
  return typeof v === 'string' && v !== '' ? v : undefined;
}
function num(v: unknown): number | undefined {
  return typeof v === 'number' && Number.isFinite(v) ? v : undefined;
}
function hexA(hex: string, a: number): string {
  const h = hex.replace('#', '');
  const full = h.length === 3 ? h.split('').map((c) => c + c).join('') : h;
  const n = parseInt(full, 16);
  return `rgba(${(n >> 16) & 255}, ${(n >> 8) & 255}, ${n & 255}, ${a})`;
}
function initials(name: string): string {
  const parts = name.split(/[-_ ]+/).filter(Boolean);
  if (parts.length >= 2) return (parts[0][0] + parts[1][0]).toUpperCase();
  return (parts[0]?.[0] ?? '?').toUpperCase();
}
function base(path: string): string {
  return path.split('/').pop() ?? path;
}

type DotState = 'done' | 'running' | 'failed' | 'pending';
function dotColor(s: DotState, mode: Mode): string {
  const d = mode === 'dark';
  return s === 'done' ? (d ? '#4ade80' : '#16a34a') : s === 'failed' ? (d ? '#f87171' : '#dc2626') : s === 'running' ? (d ? '#fbbf24' : '#d97706') : d ? '#71717a' : '#a1a1aa';
}

/** A crew-tinted avatar, keyed by the entity's codename + a role initial. */
function Avatar({ codename, label, mode }: { codename?: string; label: string; mode: Mode }) {
  const crew = codename ? parseCodename(codename).crew : '';
  const tint = (crew && crewTint(crew, mode)) || (mode === 'dark' ? '#a78bfa' : '#7c3aed');
  return (
    <div className="flex h-5 w-5 shrink-0 items-center justify-center rounded-full text-[9px] font-semibold" style={{ background: hexA(tint, 0.18), color: tint, boxShadow: `inset 0 0 0 1px ${hexA(tint, 0.4)}` }} aria-hidden>
      {initials(label)}
    </div>
  );
}

function StatusDot({ state, mode }: { state: DotState; mode: Mode }) {
  return <span className={cn('ml-auto h-1.5 w-1.5 shrink-0 rounded-full', state === 'running' && 'animate-pulse')} style={{ background: dotColor(state, mode) }} aria-label={state} />;
}

// --- round bucketing (mirrors AgentiflowGraph) -----------------------------

interface Buckets {
  leadByRound: Map<number, string>;
  byRound: Map<number, AgentiflowUnit[]>;
  maxRound: number;
  liveRound: number | null;
}

function computeBuckets(detail: AgentiflowDetail, running: boolean): Buckets {
  const { events, units, lead_transcripts } = detail;
  const roundEvs = events
    .filter((e) => e.kind === 'round')
    .map((e) => ({ round: num(e.round) ?? 0, ts: Date.parse(str(e.ts) ?? '') }))
    .sort((a, b) => a.round - b.round);
  const inProgressIdx = running ? roundEvs.length : null;
  const bucketRound = (u: AgentiflowUnit): number => {
    const t = u.started_at ? Date.parse(u.started_at) : NaN;
    if (!Number.isNaN(t)) {
      const hit = roundEvs.find((r) => !Number.isNaN(r.ts) && r.ts >= t);
      if (hit) return hit.round;
    }
    return inProgressIdx ?? (roundEvs.length ? roundEvs[roundEvs.length - 1].round : 0);
  };
  const byRound = new Map<number, AgentiflowUnit[]>();
  for (const u of units) {
    const r = bucketRound(u);
    (byRound.get(r) ?? byRound.set(r, []).get(r)!).push(u);
  }
  const leadByRound = new Map(lead_transcripts.map((t) => [t.round, t.path]));
  const maxRound = Math.max(
    roundEvs.length ? roundEvs[roundEvs.length - 1].round : -1,
    inProgressIdx ?? -1,
    ...[...byRound.keys()],
    ...[...leadByRound.keys()],
    -1,
  );
  return { leadByRound, byRound, maxRound, liveRound: running ? maxRound : null };
}

function unitDot(u: AgentiflowUnit): DotState {
  const s = unitPillStatus(u);
  return s === 'running' ? 'running' : s === 'failed' ? 'failed' : s === 'pending' ? 'pending' : 'done';
}
function stepDot(r?: { success?: boolean; skipped?: boolean }): DotState {
  if (!r) return 'pending';
  if (r.skipped) return 'done';
  if (r.success === true) return 'done';
  if (r.success === false) return 'failed';
  return 'running';
}

// ---------------------------------------------------------------------------

export default function TranscriptBrowser({ detail, running }: { detail: AgentiflowDetail; running: boolean }) {
  const mode = useThemeMode();
  const { record } = detail;
  const { leadByRound, byRound, maxRound, liveRound } = useMemo(() => computeBuckets(detail, running), [detail, running]);

  const [query, setQuery] = useState('');
  const [roundOverride, setRoundOverride] = useState<Record<number, boolean>>({});
  const [openUnits, setOpenUnits] = useState<Record<string, boolean>>({});
  const [subCache, setSubCache] = useState<Record<string, SubCache>>({});
  const [sel, setSel] = useState<Selection | null>(null);

  // Default selection: the last lead round (what the old tab showed), live
  // while the engagement runs.
  const defaultSel = useMemo<Selection | null>(() => {
    const leads = detail.lead_transcripts;
    if (leads.length === 0) return null;
    const last = leads[leads.length - 1];
    return { path: last.path, live: running, title: `Lead · round ${last.round}`, sub: `lead · round ${last.round}` };
  }, [detail.lead_transcripts, running]);
  const active = sel ?? defaultSel;

  const loadSub = useCallback((unitId: string) => {
    setSubCache((c) => (c[unitId]?.state === 'loaded' ? c : { ...c, [unitId]: { state: 'loading' } }));
    api.getRunGraph(unitId).then(
      (graph) => setSubCache((c) => ({ ...c, [unitId]: { state: 'loaded', graph } })),
      (e: unknown) => setSubCache((c) => ({ ...c, [unitId]: { state: 'error', error: e instanceof Error ? e.message : String(e) } })),
    );
  }, []);
  const toggleUnit = useCallback(
    (unitId: string) => {
      setOpenUnits((prev) => {
        const next = !prev[unitId];
        if (next) loadSub(unitId);
        return { ...prev, [unitId]: next };
      });
    },
    [loadSub],
  );

  const q = query.trim().toLowerCase();
  const matchUnit = (u: AgentiflowUnit, round: number) =>
    !q || `${u.codename} ${u.agent ?? ''} ${u.participant ?? ''} ${u.kind} round ${round}`.toLowerCase().includes(q);
  const matchLead = (round: number) => !q || `lead round ${round} ${record.codename}`.toLowerCase().includes(q);

  const rounds: number[] = [];
  for (let r = maxRound; r >= 0; r--) rounds.push(r); // newest first

  const panelKey = active ? `${active.path}:${active.live ? 1 : 0}` : 'none';

  return (
    <div className="grid grid-cols-1 gap-3 lg:grid-cols-[minmax(240px,300px)_1fr]" style={{ height: '70vh' }}>
      {/* list */}
      <div className="flex min-h-0 flex-col overflow-hidden rounded-xl border border-border bg-panel shadow-card">
        <div className="border-b border-border p-2">
          <div className="flex items-center gap-2 rounded-lg border border-border bg-bg px-2.5 py-1.5">
            <Search size={13} className="shrink-0 text-ink-mute" />
            <input
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              placeholder="Filter by codename, role, round…"
              className="min-w-0 flex-1 bg-transparent text-sm text-ink placeholder:text-ink-mute focus:outline-none"
            />
          </div>
        </div>
        <div className="min-h-0 flex-1 overflow-auto p-1.5">
          {rounds.map((r) => {
            const leadPath = leadByRound.get(r);
            const us = byRound.get(r) ?? [];
            const visUnits = us.filter((u) => matchUnit(u, r));
            const leadVisible = !!leadPath && matchLead(r);
            if (q && !leadVisible && visUnits.length === 0) return null;
            const count = (leadPath ? 1 : 0) + us.length;
            const open = q ? true : r in roundOverride ? roundOverride[r] : r === maxRound;
            return (
              <div key={r} className="mb-0.5">
                <button
                  type="button"
                  onClick={() => setRoundOverride((o) => ({ ...o, [r]: !open }))}
                  className="flex w-full items-center gap-1.5 rounded px-1.5 py-1 text-note font-medium text-ink-dim hover:text-ink"
                >
                  {open ? <ChevronDown size={13} /> : <ChevronRight size={13} />}
                  Round {r}
                  {r === liveRound && <Radio size={11} className="animate-pulse text-warn" />}
                  <span className="ml-auto text-meta tabular-nums text-ink-mute">{count}</span>
                </button>
                {open && (
                  <div>
                    {leadVisible && leadPath && (
                      <Row
                        selected={active?.path === leadPath}
                        onClick={() => setSel({ path: leadPath, live: running && r === liveRound, title: `Lead · round ${r}`, sub: `lead · round ${r}` })}
                      >
                        <Avatar codename={record.codename} label="lead" mode={mode} />
                        <AgentName codename={record.codename} derived={record.codename_derived} />
                        <span className="truncate text-meta text-ink-mute">· lead</span>
                        <StatusDot state={running && r === liveRound ? 'running' : 'done'} mode={mode} />
                      </Row>
                    )}
                    {visUnits.map((u) => (
                      <UnitRows
                        key={u.unit_id}
                        unit={u}
                        round={r}
                        mode={mode}
                        active={active}
                        expanded={!!openUnits[u.unit_id]}
                        sub={subCache[u.unit_id]}
                        onToggle={() => toggleUnit(u.unit_id)}
                        onSelect={setSel}
                      />
                    ))}
                  </div>
                )}
              </div>
            );
          })}
          {rounds.length === 0 && <p className="px-2 py-6 text-center text-sm text-ink-dim">No transcripts yet.</p>}
        </div>
      </div>

      {/* viewer */}
      <div className="flex min-h-0 flex-col overflow-hidden rounded-xl border border-border bg-panel shadow-card">
        {active ? (
          <>
            <div className="flex items-center gap-2 border-b border-border px-3 py-2">
              <span className="text-sm font-semibold text-ink">{active.title}</span>
              {active.live && (
                <span className="inline-flex items-center gap-1 text-meta text-warn">
                  <Radio size={11} className="animate-pulse" />
                  live
                </span>
              )}
              <span className="ml-auto min-w-0 truncate font-mono text-meta text-ink-mute" title={active.path}>
                {base(active.path)}
              </span>
            </div>
            <div className="min-h-0 flex-1 overflow-auto">
              <TranscriptPanel key={panelKey} path={active.path} live={active.live} />
            </div>
          </>
        ) : (
          <div className="flex flex-1 items-center justify-center p-8 text-center text-sm text-ink-dim">Select a transcript to view it.</div>
        )}
      </div>
    </div>
  );
}

function Row({ selected, onClick, children }: { selected: boolean; onClick: () => void; children: React.ReactNode }) {
  return (
    <button
      type="button"
      onClick={onClick}
      className={cn('flex w-full items-center gap-2 rounded-md px-2 py-1 text-left text-note', selected ? 'bg-brand-500/15 text-ink' : 'text-ink-dim hover:bg-surface')}
    >
      {children}
    </button>
  );
}

function UnitRows({
  unit,
  round,
  mode,
  active,
  expanded,
  sub,
  onToggle,
  onSelect,
}: {
  unit: AgentiflowUnit;
  round: number;
  mode: Mode;
  active: Selection | null;
  expanded: boolean;
  sub?: SubCache;
  onToggle: () => void;
  onSelect: (s: Selection) => void;
}) {
  const isWorkflow = unit.kind === 'workflow';
  const roleWord = parseCodename(unit.codename).role;
  const who = [roleWord, unit.agent].filter(Boolean).join(' · ') || unit.codename;
  return (
    <>
      <div className="flex items-center">
        {isWorkflow && (
          <button type="button" onClick={onToggle} className="ml-0.5 shrink-0 rounded text-ink-mute hover:text-ink" aria-label={expanded ? 'Collapse workflow' : 'Expand workflow'}>
            {expanded ? <ChevronDown size={12} /> : <ChevronRight size={12} />}
          </button>
        )}
        <div className={cn('min-w-0 flex-1', !isWorkflow && 'pl-[18px]')}>
          <Row
            selected={!!unit.transcript_path && active?.path === unit.transcript_path}
            onClick={() => {
              if (unit.transcript_path) onSelect({ path: unit.transcript_path, live: false, title: who, sub: `round ${round}` });
              else if (isWorkflow) onToggle();
            }}
          >
            <Avatar codename={unit.codename} label={unit.agent ?? roleWord ?? unit.codename} mode={mode} />
            <AgentName codename={unit.codename} agent={unit.agent ?? undefined} derived={unit.codename_derived} />
            {isWorkflow && <span className="shrink-0 rounded bg-surface px-1 text-[10px] font-medium text-ink-mute ring-1 ring-inset ring-border">flow</span>}
            <StatusDot state={unitDot(unit)} mode={mode} />
          </Row>
        </div>
      </div>
      {isWorkflow && expanded && (
        <div className="ml-[22px] border-l border-border pl-1.5">
          {!sub || sub.state === 'loading' ? (
            <p className="py-1 pl-2 text-meta text-ink-mute">loading flow…</p>
          ) : sub.state === 'error' ? (
            <p className="py-1 pl-2 text-meta text-err">couldn't load this flow</p>
          ) : (
            <WorkflowSteps graph={sub.graph!} mode={mode} active={active} onSelect={onSelect} round={round} />
          )}
        </div>
      )}
    </>
  );
}

function WorkflowSteps({
  graph,
  mode,
  active,
  onSelect,
  round,
}: {
  graph: RunGraphResponse;
  mode: Mode;
  active: Selection | null;
  onSelect: (s: Selection) => void;
  round: number;
}) {
  const resultByStep = new Map((graph.step_results ?? []).map((r) => [r.step_id, r]));
  const withPath = (graph.step_results ?? []).filter((r) => str(r.transcript_path));
  if (withPath.length === 0) return <p className="py-1 pl-2 text-meta text-ink-mute">no step transcripts</p>;
  return (
    <>
      {(graph.workflow?.steps ?? []).map((s: StepNodeDto) => {
        const r = resultByStep.get(s.id);
        const path = str(r?.transcript_path);
        if (!path) return null;
        const label = `${s.id}${s.agent ? ` · ${s.agent}` : ''}`;
        return (
          <Row key={s.id} selected={active?.path === path} onClick={() => onSelect({ path, live: false, title: `${s.id}${s.agent ? ` · ${s.agent}` : ''}`, sub: `workflow step · round ${round}` })}>
            {r?.codename ? <AgentName codename={r.codename} agent={s.agent ?? undefined} derived={r.codename_derived} /> : <span className="truncate text-meta">{label}</span>}
            <StatusDot state={stepDot(r)} mode={mode} />
          </Row>
        );
      })}
    </>
  );
}
