# Session context snapshot

`GET /api/sessions/:id/context?host=local` returns a bounded, read-only observation for the selected session's lead conversation. The local host advertises `session.context.v1`. Unsupported remote hosts return 501 rather than reading a similarly named local session. Both active and archived sessions are readable; missing sessions return 404 and mismatched stored identity returns 409.

This is separate from session accounting. The usage timeline includes dispatched runs, whereas context pressure must describe the lead. Only the selected active run (or newest recorded lead run) is read. Its RunStart must match the run ID, workspace, agent, provider and model. The reader never follows dispatch references or falls back to an older run when the newest one has no observation. Normal Usage records provide input tokens; summarizer usage is excluded. A new inline Compaction invalidates the previous input until another normal usage record. Cumulative session tokens are never used.

The response is an object with `schema_version: 1`, `session_id`, `host_id: "local"`, and these fields (unknown values are JSON null):

| Field | Meaning |
|---|---|
| `input_tokens` | Latest nonzero normal lead-call input observation (zero means unreported). This is Rupu-normalized `Event::Usage.input_tokens`, the same input used by the CLI context gauge; cache reads/writes are included as subsets, not subtracted as billed uncached tokens. |
| `context_window_tokens` | Runtime `effective_context_window()`, stored resolved/learned limit before agent pin. |
| `context_window_source` | `agent`, `config`, `live`, `observed`, `unknown`, or null. |
| `context_window_source_detail` | Original typed limit source, including cached `fetched_at` and `stale` when available. |
| `compact_at_percent` | Effective runtime percent clamped to its supported range. |
| `compact_threshold_tokens` | Runtime `compaction_threshold(None)`, including output headroom. |
| `output_headroom_tokens`, `output_headroom_source` | Known shared-window output allowance considered by that threshold; null when absent, not shared, or inapplicable. |
| `freshness` | See states below. |
| `compacting` | A recorded compaction process is currently alive. Reading does not clean stale markers. |
| `run_id` | Exact owning lead run, when identified. |
| `transcript_turn` | Recorded TurnStart index for this observation, when present. |
| `usage_index` | One-based ordinal of normal Usage records in this lead transcript. |
| `call_id`, `observed_at` | Null on the current transcript schema: no model-call ID or per-Usage timestamp is recorded. |
| `basis` | `latest_recorded_lead_input`. This is not an estimate of the next prompt. |

`freshness` is `recorded`, `awaiting_usage`, `after_compaction`, `compacting`, `model_changed`, `unavailable`, or `changed_during_read`. Consumers may compute pressure only when `recorded` and both the input observation and nonzero window exist. `recorded` does not mean the next prompt has that size. Settings that select another model/provider clear the window and measurement until the runtime records the new model. Unknown limits stay unknown; this endpoint never discovers models or calls a provider.

Manual compaction can rewrite history without a per-usage timestamp. In that case the previous measurement remains invalid until a lead run starts after the recorded successful compaction. Malformed compaction metadata remains unavailable. A concurrently changed session, settings, compaction receipt or transcript invalidates the observation rather than combining versions. Transcript reads are capped at 64 MiB and 250,000 parsed events, including concurrent growth, and happen on the blocking pool. Other source records use the same 64 MiB byte limit. No history, prompts, transcript paths or tool output are returned.

Compact remains a separate explicit action through the existing session command/control contract. This endpoint does not compact, mutate settings, create a turn or retry a command.
