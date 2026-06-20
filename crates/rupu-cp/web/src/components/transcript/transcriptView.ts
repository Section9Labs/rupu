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
 *   • each tool is classified into a `ToolKind` from its tool name.
 *   • tools are grouped into turns, a new turn starting at each
 *     `assistant_message` (tools before the first assistant land in a leading
 *     turn with no assistant).
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
}

export interface TurnView {
  assistant?: { content: string; thinking?: string };
  tools: ToolView[];
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
  // The turn tools currently attach to. Created lazily so a leading turn only
  // appears when there are tools before the first assistant_message.
  let current: TurnView | null = null;
  // Tool lookup by call_id, so a later `tool_result` finds its `tool_call`.
  const toolByCall = new Map<string, ToolView>();
  // The most recent diff-/terminal-expecting tools awaiting their paired
  // `file_edit` / `command_run` (matched by adjacency).
  let pendingDiff: ToolView | null = null;
  let pendingTerminal: ToolView | null = null;

  function ensureTurn(): TurnView {
    if (current === null) {
      current = { tools: [], summary: { toolCount: 0, findingCount: 0, result: 'running' } };
      turns.push(current);
    }
    return current;
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

      case 'assistant_message': {
        const turn: TurnView = {
          assistant: {
            content: asString(data.content) ?? '',
            ...(asString(data.thinking) !== null
              ? { thinking: asString(data.thinking) as string }
              : {}),
          },
          tools: [],
          summary: { toolCount: 0, findingCount: 0, result: 'running' },
        };
        turns.push(turn);
        current = turn;
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

        ensureTurn().tools.push(view);
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
        }
        // An unpaired result carries no tool_call to render against; ignore.
        break;
      }

      case 'file_edit': {
        if (pendingDiff) {
          pendingDiff.diff = {
            path: asString(data.path) ?? '',
            editKind: asString(data.kind) ?? '',
            diff: asString(data.diff) ?? '',
          };
          pendingDiff = null;
        }
        break;
      }

      case 'command_run': {
        if (pendingTerminal) {
          const argv = Array.isArray(data.argv) ? data.argv : [];
          const command = typeof argv[2] === 'string' ? argv[2] : '';
          pendingTerminal.terminal = {
            command,
            cwd: asString(data.cwd) ?? '',
            exitCode: asNumber(data.exit_code) ?? 0,
          };
          pendingTerminal = null;
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

      // user_message and action_emitted are dead/legacy shapes — findings come
      // from `report_finding` tool_calls now, and there is no user_message in
      // the live stream. turn_start / turn_end / assistant_delta /
      // gate_requested carry no render payload. All ignored gracefully.
      default:
        break;
    }
  }

  // Finalize per-turn summaries.
  for (const turn of turns) {
    const toolCount = turn.tools.length;
    const findingCount = turn.tools.filter((t) => t.kind === 'finding').length;
    const hasError = turn.tools.some((t) => t.error !== undefined);
    turn.summary = {
      toolCount,
      findingCount,
      result: hasError ? 'error' : sawRunComplete ? 'ok' : 'running',
    };
  }

  return { header, turns, footer };
}
