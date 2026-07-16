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
  | { type: 'run_start'; data: { run_id: string; workspace_id?: string; agent: string; provider: string; model: string; started_at: string; mode: string } }
  | { type: 'turn_start'; data: Record<string, unknown> }
  | { type: 'assistant_delta'; data: { content: string } }
  | { type: 'assistant_message'; data: { content: string; thinking?: string | null } }
  | { type: 'tool_call'; data: { call_id: string; tool: string; input: unknown } }
  | { type: 'tool_result'; data: { call_id: string; output: string; error?: string | null; duration_ms: number; structured?: unknown } }
  | { type: 'file_edit'; data: Record<string, unknown> }
  | { type: 'command_run'; data: Record<string, unknown> }
  | { type: 'action_emitted'; data: Record<string, unknown> }
  | { type: 'gate_requested'; data: Record<string, unknown> }
  | { type: 'turn_end'; data: { tokens_in?: number | null; tokens_out?: number | null } }
  | { type: 'usage'; data: { input_tokens: number; output_tokens: number; cached_tokens: number } }
  | { type: 'run_complete'; data: { run_id: string; status: string; total_tokens: number; duration_ms: number; error?: string | null } }
  | { type: string; data: Record<string, unknown> }; // catch-all for forward-compat

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
}

export interface TranscriptResponse {
  events: TranscriptEvent[];
  summary: TranscriptSummary | null;
}
