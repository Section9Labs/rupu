/**
 * Pure event → view mapping for the agent transcript panel.
 *
 * Turns the flat `TranscriptEvent[]` stream (adjacently-tagged) into an ordered
 * render model the `TranscriptPanel` component can paint without re-deriving
 * structure. All pairing / classification logic lives here so it stays testable:
 *
 *   • `tool_result` is paired to its `tool_call` by `call_id`
 *     (output / error / durationMs ride onto the ToolView).
 *   • the next `file_edit` is paired (by adjacency) onto the preceding
 *     `write_file` / `edit_file` tool; the next `command_run` onto the
 *     preceding `bash` tool.
 *   • findings are built from `report_finding` tool_calls (NOT `action_emitted`).
 *   • `tool_audit` (step `actions:` enforcement's audit trail) is paired onto
 *     a `tool_call` of the SAME tool name, FIFO per name — NOT by adjacency.
 *     A single turn can carry >1 `tool_use` block; `run_agent` writes every
 *     `tool_call` for the turn before dispatching any of them, so on disk the
 *     order is `call A, call B, audit A, result A, audit B, result B` — a
 *     "last tool_call wins" adjacency scheme would attach A's audit to B's
 *     card and orphan B's audit into a phantom standalone card. Matching by
 *     `data.tool` (present on every `tool_audit`) against a FIFO queue keyed
 *     by tool name gets each audit onto its own call, including when the same
 *     tool is called twice in one turn. When the queue for that name is empty
 *     (an action-node call has no `tool_call`/`tool_result` shape at all),
 *     the audit is surfaced as a standalone entry so it's never silently
 *     dropped.
 *   • each tool is classified into a `ToolKind` from its tool name.
 *   • events land as ordered `TurnBlock`s inside a `TurnView`, a new turn
 *     starting at each `turn_start` (v2 transcripts). Legacy v1 transcripts
 *     carry no `turn_start` at all — for those, `assistant_message` itself
 *     opens a new turn (tools/thinking before the first assistant land in a
 *     leading turn with no assistant block), matching the old per-assistant
 *     grouping exactly.
 *   • `thinking` (v2) becomes its own block; a legacy `assistant_message
 *     .thinking` string becomes a `thinking` block emitted immediately
 *     before that turn's `assistant` block, so both eras render the same
 *     block shape.
 *   • `gate_requested` / `seed` / `user_message` / `notice` / `compaction`
 *     each become their own block kind. Anything else unrecognised becomes
 *     an `unknown` block carrying the raw event `type` — nothing is ever
 *     silently dropped, EXCEPT `net_flow`: TranscriptSink appends one into
 *     this same JSONL on essentially every provider call, and it already has
 *     a dedicated display (the RunDetail Netflow tab, fed from the separate
 *     per-run ledger API) — rendering it here too would flood nearly every
 *     turn with a redundant/misleading row, so it's a deliberate no-op.
 *   • orphaned pairing targets (a `tool_result`/`file_edit`/`command_run`
 *     with no preceding `tool_call` to attach to in this snapshot) render as
 *     a standalone `tool` block instead of vanishing.
 *   • a header is surfaced from `run_start`; a footer from `run_complete`,
 *     falling back to the last `usage` event when the run hasn't completed.
 *
 * No React, no DOM — a deterministic function over the event list.
 */

import type { TranscriptEvent } from '../../lib/transcript';

// ---------------------------------------------------------------------------
// View model
// ---------------------------------------------------------------------------

export interface TranscriptHeader {
  agent: string;
  model: string;
  provider: string;
  mode: string;
  startedAt: string;
}

export interface TranscriptFooter {
  /** Run status when known (`run_complete`), else null (still running). */
  status: string | null;
  totalTokens: number | null;
  durationMs: number | null;
  error: string | null;
}

export type Severity = 'info' | 'low' | 'medium' | 'high' | 'critical';

export interface FindingView {
  severity: Severity;
  summary: string;
  scope: string;
  filePath?: string;
  lineRange?: [number, number];
  concernId?: string;
  rationale: string;
  codeExcerpt?: string;
  references: string[];
}

export type ToolKind =
  | 'finding'
  | 'read'
  | 'grep'
  | 'glob'
  | 'diff'
  | 'terminal'
  | 'subrun'
  | 'coverage'
  | 'ast_grep'
  | 'generic';

export interface ToolView {
  callId?: string;
  tool: string;
  input: unknown;
  output?: string;
  error?: string;
  durationMs?: number;
  kind: ToolKind;
  /** kind === 'finding' */
  finding?: FindingView;
  /** kind === 'diff' (from the paired `file_edit`). */
  diff?: { path: string; editKind: string; diff: string };
  /** kind === 'terminal' (from the paired `command_run`). */
  terminal?: { command: string; cwd: string; exitCode: number };
  /** kind === 'ast_grep' (from the paired `tool_result.data.structured`). */
  structured?: unknown;
  /**
   * The `tool_audit` record for this call, when one was emitted (step
   * `actions:` enforcement, 2026-07-26 design). Paired onto a `tool_call`
   * of the SAME tool name via a FIFO queue (see the module doc) — never by
   * adjacency, since a turn can carry >1 `tool_use` block and all of a
   * turn's `tool_call`s are written before any of their audits/results.
   */
  audit?: ToolAuditView;
}

export interface ToolAuditView {
  /** In the step's `actions:` allowlist. `false` also when the step
   * is unrestricted — see `restricted`. */
  declared: boolean;
  /** Covered by the agent's `tools:` grant (pre-narrowing). */
  granted: boolean;
  /** The call was actually denied. */
  blocked: boolean;
  /** The step declared a non-empty `actions:` allowlist at all
   * (disambiguates `declared: false`). */
  restricted: boolean;
}

export type TurnBlock =
  | { kind: 'assistant'; content: string }
  | { kind: 'thinking'; text: string | null; provider: string } // text null = redacted
  | { kind: 'user'; content: string }
  | { kind: 'tool'; view: ToolView }
  | { kind: 'gate'; gateId: string; prompt: string; decision: string | null; decidedBy: string | null }
  | { kind: 'seed'; messageCount: number; sourceTranscript: string | null }
  | { kind: 'notice'; noticeKind: string; message: string }
  | { kind: 'compaction'; seq: number; summarized: number }
  | { kind: 'unknown'; type: string };

export interface TurnView {
  blocks: TurnBlock[];
  tokensIn: number | null;
  tokensOut: number | null;
  summary: {
    toolCount: number;
    findingCount: number;
    result: 'ok' | 'error' | 'running';
  };
}

export interface TranscriptView {
  header: TranscriptHeader | null;
  turns: TurnView[];
  footer: TranscriptFooter | null;
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function asString(v: unknown): string | null {
  return typeof v === 'string' ? v : null;
}

function asNumber(v: unknown): number | null {
  return typeof v === 'number' ? v : null;
}

function asRecord(v: unknown): Record<string, unknown> | null {
  return typeof v === 'object' && v !== null && !Array.isArray(v)
    ? (v as Record<string, unknown>)
    : null;
}

const SEVERITIES: ReadonlySet<string> = new Set([
  'info',
  'low',
  'medium',
  'high',
  'critical',
]);

function asSeverity(v: unknown): Severity {
  return typeof v === 'string' && SEVERITIES.has(v) ? (v as Severity) : 'info';
}

function asLineRange(v: unknown): [number, number] | undefined {
  if (Array.isArray(v) && v.length === 2 && typeof v[0] === 'number' && typeof v[1] === 'number') {
    return [v[0], v[1]];
  }
  return undefined;
}

function asStringArray(v: unknown): string[] {
  if (!Array.isArray(v)) return [];
  return v.filter((x): x is string => typeof x === 'string');
}

/**
 * Parse a `report_finding` tool_call input into a FindingView.
 * Returns null when the shape isn't a recognisable finding.
 */
function asFinding(input: unknown): FindingView | null {
  const rec = asRecord(input);
  if (!rec) return null;
  const evidence = asRecord(rec.evidence) ?? {};
  const summary = asString(rec.summary);
  const rationale = asString(evidence.rationale);
  // A finding must at least carry a summary or a rationale to be meaningful.
  if (summary === null && rationale === null) return null;

  const finding: FindingView = {
    severity: asSeverity(rec.severity),
    summary: summary ?? '',
    scope: asString(rec.scope) ?? '',
    rationale: rationale ?? '',
    references: asStringArray(evidence.references),
  };
  const filePath = asString(rec.file_path);
  if (filePath !== null) finding.filePath = filePath;
  const lineRange = asLineRange(rec.line_range);
  if (lineRange !== undefined) finding.lineRange = lineRange;
  const concernId = asString(rec.concern_id);
  if (concernId !== null) finding.concernId = concernId;
  const codeExcerpt = asString(evidence.code_excerpt);
  if (codeExcerpt !== null) finding.codeExcerpt = codeExcerpt;
  return finding;
}

/** Classify a tool by its name. `report_finding` is resolved separately. */
function classify(tool: string): ToolKind {
  switch (tool) {
    case 'report_finding':
      return 'finding';
    case 'read_file':
      return 'read';
    case 'grep':
      return 'grep';
    case 'glob':
      return 'glob';
    case 'write_file':
    case 'edit_file':
      return 'diff';
    case 'bash':
      return 'terminal';
    case 'dispatch_agent':
    case 'dispatch_agents_parallel':
      return 'subrun';
    case 'ast_grep':
      return 'ast_grep';
    default:
      return tool.startsWith('coverage_') ? 'coverage' : 'generic';
  }
}

// ---------------------------------------------------------------------------
// Builder
// ---------------------------------------------------------------------------

export function buildTranscriptView(events: TranscriptEvent[]): TranscriptView {
  let header: TranscriptHeader | null = null;
  let footer: TranscriptFooter | null = null;
  let sawRunComplete = false;

  const turns: TurnView[] = [];
  // The turn new blocks currently attach to. Created lazily (via `ensureTurn`)
  // so a leading turn only appears when something lands before the first
  // `turn_start` / `assistant_message`.
  let current: TurnView | null = null;
  // Tool lookup by call_id, so a later `tool_result` finds its `tool_call`.
  const toolByCall = new Map<string, ToolView>();
  // The most recent diff-/terminal-expecting tools awaiting their paired
  // `file_edit` / `command_run` (matched by adjacency).
  let pendingDiff: ToolView | null = null;
  let pendingTerminal: ToolView | null = null;
  // Tool_calls awaiting their `tool_audit` line, queued FIFO per tool NAME
  // (not a single slot — see the module doc for why adjacency/last-call-wins
  // misattributes when a turn has >1 tool_use of possibly-mixed names).
  const pendingAuditsByTool = new Map<string, ToolView[]>();
  // ISSUES.md I-40: an action node emits `action_emitted` (carrying the
  // RENDERED `with:` args) immediately before its `tool_audit`, and has no
  // `tool_call`/`tool_result` pair at all. Stash the args here so the
  // standalone entry the `tool_audit` arm creates can show what the action
  // actually sent, instead of `input: undefined`. Keyed by tool name, queued
  // because a step may invoke the same tool more than once.
  const pendingActionArgsByTool = new Map<string, unknown[]>();

  // Legacy (v1) transcripts carry no `turn_start` at all — for those,
  // `assistant_message` itself opens a new turn, matching the old
  // per-assistant grouping. v2 transcripts group by `turn_start` instead;
  // once we've seen one, `assistant_message` just lands in the current turn.
  let sawTurnStart = false;

  function newTurn(): TurnView {
    const t: TurnView = {
      blocks: [],
      tokensIn: null,
      tokensOut: null,
      summary: { toolCount: 0, findingCount: 0, result: 'running' },
    };
    turns.push(t);
    current = t;
    return t;
  }

  function ensureTurn(): TurnView {
    return current ?? newTurn();
  }

  for (const ev of events) {
    const data = (ev.data ?? {}) as Record<string, unknown>;

    switch (ev.type) {
      case 'run_start': {
        header = {
          agent: asString(data.agent) ?? '',
          model: asString(data.model) ?? '',
          provider: asString(data.provider) ?? '',
          mode: asString(data.mode) ?? '',
          startedAt: asString(data.started_at) ?? '',
        };
        break;
      }

      case 'turn_start': {
        sawTurnStart = true;
        newTurn();
        break;
      }

      case 'turn_end': {
        const t = ensureTurn();
        t.tokensIn = asNumber(data.tokens_in);
        t.tokensOut = asNumber(data.tokens_out);
        break;
      }

      case 'thinking': {
        ensureTurn().blocks.push({
          kind: 'thinking',
          text: asString(data.text),
          provider: asString(data.provider) ?? '',
        });
        break;
      }

      case 'assistant_delta':
      case 'thinking_delta':
        // Consolidated events (`assistant_message` / `thinking`) carry the
        // same content already — the deltas are for live-streaming UIs only.
        break;

      case 'net_flow':
        // Deliberately unrendered here. TranscriptSink appends a `net_flow`
        // line into this same JSONL on essentially every provider call, so
        // treating it like an unrecognised event would flood nearly every
        // turn with a misleading "unrecognized event" row. Netflow has its
        // own dedicated display — the RunDetail Netflow tab, fed from the
        // separate per-run ledger API — so it's a deliberate no-op here.
        break;

      case 'assistant_message': {
        // v1 files have no reliable turn framing convention here: keep the
        // legacy "assistant opens a turn" grouping unless turn_start is
        // doing the grouping.
        const turn = sawTurnStart ? ensureTurn() : newTurn();
        const legacyThinking = asString(data.thinking);
        if (legacyThinking !== null) {
          turn.blocks.push({ kind: 'thinking', text: legacyThinking, provider: '' });
        }
        turn.blocks.push({ kind: 'assistant', content: asString(data.content) ?? '' });
        break;
      }

      case 'user_message': {
        ensureTurn().blocks.push({ kind: 'user', content: asString(data.content) ?? '' });
        break;
      }

      case 'seed': {
        ensureTurn().blocks.push({
          kind: 'seed',
          messageCount: asNumber(data.message_count) ?? 0,
          sourceTranscript: asString(data.source_transcript),
        });
        break;
      }

      case 'gate_requested': {
        ensureTurn().blocks.push({
          kind: 'gate',
          gateId: asString(data.gate_id) ?? '',
          prompt: asString(data.prompt) ?? '',
          decision: asString(data.decision),
          decidedBy: asString(data.decided_by),
        });
        break;
      }

      case 'notice': {
        ensureTurn().blocks.push({
          kind: 'notice',
          noticeKind: asString(data.kind) ?? '',
          message: asString(data.message) ?? '',
        });
        break;
      }

      case 'compaction': {
        ensureTurn().blocks.push({
          kind: 'compaction',
          seq: asNumber(data.seq) ?? 0,
          summarized: asNumber(data.summarized_messages) ?? 0,
        });
        break;
      }

      case 'tool_call': {
        const tool = asString(data.tool) ?? '';
        const kind = classify(tool);
        const view: ToolView = {
          tool,
          input: data.input,
          kind,
        };
        const callId = asString(data.call_id);
        if (callId !== null) {
          view.callId = callId;
          toolByCall.set(callId, view);
        }
        if (kind === 'finding') {
          const finding = asFinding(data.input);
          if (finding) view.finding = finding;
        }
        // Arm adjacency pairing for the next file_edit / command_run.
        pendingDiff = kind === 'diff' ? view : null;
        pendingTerminal = kind === 'terminal' ? view : null;
        // Queue this call for a possible tool_audit line, FIFO per tool
        // name (agent tool calls only — action-node calls have no
        // tool_call at all, see the tool_audit case's standalone fallback).
        const queue = pendingAuditsByTool.get(tool);
        if (queue) {
          queue.push(view);
        } else {
          pendingAuditsByTool.set(tool, [view]);
        }

        ensureTurn().blocks.push({ kind: 'tool', view });
        break;
      }

      case 'tool_result': {
        const callId = asString(data.call_id) ?? '';
        const view = toolByCall.get(callId);
        if (view) {
          const output = asString(data.output);
          if (output !== null) view.output = output;
          const error = asString(data.error);
          if (error !== null) view.error = error;
          const durationMs = asNumber(data.duration_ms);
          if (durationMs !== null) view.durationMs = durationMs;
          if (data.structured !== undefined) view.structured = data.structured;
        } else {
          // Orphan result (no tool_call in this snapshot): standalone card.
          const orphan: ToolView = { tool: '(result)', input: undefined, kind: 'generic' };
          if (callId !== '') orphan.callId = callId;
          const output = asString(data.output);
          if (output !== null) orphan.output = output;
          const error = asString(data.error);
          if (error !== null) orphan.error = error;
          const durationMs = asNumber(data.duration_ms);
          if (durationMs !== null) orphan.durationMs = durationMs;
          ensureTurn().blocks.push({ kind: 'tool', view: orphan });
        }
        break;
      }

      case 'tool_audit': {
        const audit: ToolAuditView = {
          declared: data.declared === true,
          granted: data.granted === true,
          blocked: data.blocked === true,
          restricted: data.restricted === true,
        };
        const tool = asString(data.tool) ?? '';
        const queue = pendingAuditsByTool.get(tool);
        const target = queue && queue.length > 0 ? queue.shift() : undefined;
        if (target) {
          target.audit = audit;
        } else {
          // No queued tool_call of this name to attach to — this is an
          // action-node call (execute_action_step's tool_audit has no
          // accompanying tool_call/tool_result shape at all). Surface
          // it as a standalone entry so the audit line is never
          // silently dropped.
          //
          // I-40: attach the rendered `with:` args stashed by the matching
          // `action_emitted` that precedes it, so the entry shows what the
          // action actually sent. Still exactly ONE entry per action call —
          // the two events are merged, never surfaced separately.
          const argQueue = pendingActionArgsByTool.get(tool);
          const input = argQueue && argQueue.length > 0 ? argQueue.shift() : undefined;
          ensureTurn().blocks.push({
            kind: 'tool',
            view: { tool, input, kind: classify(tool), audit },
          });
        }
        break;
      }

      case 'file_edit': {
        const diff = {
          path: asString(data.path) ?? '',
          editKind: asString(data.kind) ?? '',
          diff: asString(data.diff) ?? '',
        };
        if (pendingDiff) {
          pendingDiff.diff = diff;
          pendingDiff = null;
        } else {
          // Orphan edit (no preceding write_file/edit_file tool_call in this
          // snapshot): standalone card instead of a silently dropped diff.
          ensureTurn().blocks.push({
            kind: 'tool',
            view: { tool: 'file_edit', input: undefined, kind: 'diff', diff },
          });
        }
        break;
      }

      case 'command_run': {
        const argv = Array.isArray(data.argv) ? data.argv : [];
        const command = typeof argv[2] === 'string' ? argv[2] : '';
        const terminal = {
          command,
          cwd: asString(data.cwd) ?? '',
          exitCode: asNumber(data.exit_code) ?? 0,
        };
        if (pendingTerminal) {
          pendingTerminal.terminal = terminal;
          pendingTerminal = null;
        } else {
          // Orphan command (no preceding bash tool_call in this snapshot):
          // standalone card instead of a silently dropped terminal block.
          ensureTurn().blocks.push({
            kind: 'tool',
            view: { tool: 'command_run', input: undefined, kind: 'terminal', terminal },
          });
        }
        break;
      }

      case 'usage': {
        const input = asNumber(data.input_tokens) ?? 0;
        const output = asNumber(data.output_tokens) ?? 0;
        if (!footer) {
          footer = { status: null, totalTokens: input + output, durationMs: null, error: null };
        } else if (footer.totalTokens === null) {
          footer.totalTokens = input + output;
        }
        break;
      }

      case 'run_complete': {
        sawRunComplete = true;
        footer = {
          status: asString(data.status),
          totalTokens: asNumber(data.total_tokens),
          durationMs: asNumber(data.duration_ms),
          error: asString(data.error),
        };
        break;
      }

      case 'action_emitted': {
        // Two distinct shapes share this event name.
        //
        // 1. The LEGACY finding shape (`{ action: 'report_finding', severity,
        //    summary }`) really is dead — findings come from `report_finding`
        //    tool_calls now. Ignored, as before.
        // 2. The ACTION-NODE shape (`{ kind, payload, allowed, applied }`),
        //    written by `execute_action_step` just before its `tool_audit`.
        //    `payload` is the rendered `with:` args (ISSUES.md I-40).
        //
        // Only shape 2 is stashed, and it deliberately produces NO item of its
        // own — the following `tool_audit` picks the args up and renders a
        // single merged entry. That preserves the standing invariant that an
        // action call is never surfaced twice.
        const kind = asString(data.kind);
        if (kind && data.payload !== undefined) {
          const queue = pendingActionArgsByTool.get(kind);
          if (queue) queue.push(data.payload);
          else pendingActionArgsByTool.set(kind, [data.payload]);
        }
        break;
      }

      default: {
        // Anything unrecognised (including forward-compat event types the
        // catch-all member of TranscriptEvent admits) still renders, rather
        // than vanishing silently.
        ensureTurn().blocks.push({ kind: 'unknown', type: ev.type });
        break;
      }
    }
  }

  // Finalize per-turn summaries.
  for (const turn of turns) {
    const tools = turn.blocks.filter(
      (b): b is Extract<TurnBlock, { kind: 'tool' }> => b.kind === 'tool',
    );
    const hasError = tools.some((t) => t.view.error !== undefined);
    turn.summary = {
      toolCount: tools.length,
      findingCount: tools.filter((t) => t.view.kind === 'finding').length,
      result: hasError ? 'error' : sawRunComplete ? 'ok' : 'running',
    };
  }

  // Drop turns that ended up with no blocks (e.g. a turn_start whose turn
  // only produced deltas) so empty shells don't render.
  const nonEmpty = turns.filter((t) => t.blocks.length > 0);

  return { header, turns: nonEmpty, footer };
}
