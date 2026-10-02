/**
 * Transcript event types for rupu agent runs.
 *
 * The backend serializes `rupu_transcript::Event` with adjacently-tagged
 * serde: `{"type":"tool_call","data":{...}}` (tag="type", content="data").
 */

// ---------------------------------------------------------------------------
// Adjacently-tagged event union
// ---------------------------------------------------------------------------

export type TranscriptEvent =
  | { type: 'run_start'; data: { run_id: string; workspace_id?: string; agent: string; provider: string; model: string; started_at: string; mode: string; codename?: string } }
  | { type: 'turn_start'; data: Record<string, unknown> }
  | { type: 'assistant_delta'; data: { content: string } }
  | { type: 'assistant_message'; data: { content: string; thinking?: string | null } }
  | { type: 'tool_call'; data: { call_id: string; tool: string; input: unknown } }
  | { type: 'tool_result'; data: { call_id: string; output: string; error?: string | null; duration_ms: number; structured?: unknown } }
  | { type: 'file_edit'; data: Record<string, unknown> }
  | { type: 'command_run'; data: Record<string, unknown> }
  | { type: 'action_emitted'; data: Record<string, unknown> }
  | { type: 'tool_audit'; data: { tool: string; declared: boolean; granted: boolean; blocked: boolean; restricted: boolean } }
  | { type: 'gate_requested'; data: Record<string, unknown> }
  | { type: 'turn_end'; data: { tokens_in?: number | null; tokens_out?: number | null } }
  | { type: 'usage'; data: { input_tokens: number; output_tokens: number; cached_tokens: number } }
  | { type: 'run_complete'; data: { run_id: string; status: string; total_tokens: number; duration_ms: number; error?: string | null } }
  | { type: 'thinking'; data: { text?: string | null; provider: string; model: string; raw?: unknown } }
  | { type: 'thinking_delta'; data: { content: string } }
  | { type: 'user_message'; data: { content: string } }
  | { type: 'seed'; data: { message_count: number; sha256?: string; source_transcript?: string | null; messages?: unknown } }
  | { type: 'notice'; data: { kind: string; message: string } }
  | { type: 'compaction'; data: { seq: number; summarized_messages: number; backup_path?: string; messages?: unknown } }
  | { type: 'net_flow'; data: { flow: unknown } }
  | { type: 'outcome'; data: { turn_idx: number; outcome: OutcomeRecord } }
  | { type: 'recovery'; data: RecoveryData }
  // Catch-all for forward-compat: the server passes an unrecognized event's
  // `data` through verbatim; the view model keeps it on the `unknown` block.
  | { type: string; data: Record<string, unknown> };

export type OutcomeSeverity = 'info' | 'warning' | 'error';

/** `rupu_transcript::OutcomeRecord`. */
export interface OutcomeRecord {
  id: string;
  class: string;
  severity: OutcomeSeverity;
  title: string;
  detail?: string | null;
  error_class?: string | null;
  wire?: unknown;
}

/** `Event::Recovery`'s payload. `action` is a snake_case name; an action this
 *  build does not know still renders (as its own name). */
export interface RecoveryData {
  outcome_id: string;
  rung: number;
  action: string;
  attempt?: number | null;
  budget?: number | null;
  provider?: string | null;
  model?: string | null;
  reason?: string | null;
  merge_into_previous?: boolean;
  continues_output?: boolean;
}

// ---------------------------------------------------------------------------
// Transcript summary / response shapes
// ---------------------------------------------------------------------------

export interface TranscriptSummary {
  run_id: string;
  agent: string;
  provider: string;
  model: string;
  status: string;
  total_tokens: number;
  duration_ms: number;
  started_at: string;
  error?: string | null;
  codename?: string;
}

export interface TranscriptResponse {
  events: TranscriptEvent[];
  summary: TranscriptSummary | null;
  /** Lines the server could not parse (absent on older servers). */
  unparsed?: number;
  /** The coordinator could not collect the rest of this transcript from
   *  its host (spec §4.2). Absent unless true. */
  partial?: boolean;
}
