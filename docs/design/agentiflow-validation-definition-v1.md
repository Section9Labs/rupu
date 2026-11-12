# Agentiflow validation definition echo

Feature: `agentiflow.validation_definition.v1`. This additive, local-only authoring capability uses the existing `POST /api/agentiflow-defs/validate` route and authoritative `AgentiflowDef` parser/check. It performs no write or launch and does not change execution policy.

## Opt-in contract

Request: `{raw: String, scope_id?: String|null, include_definition: true}`. Omit the flag or set it false to retain the exact existing validation response and behavior. Scope IDs continue to resolve registered workspaces and their profile overlays; remote host selection remains unsupported. No display-name or current-project inference is added.

A successful opted-in response retains `ok`, `name`, `lead`, and `engagement_profiles`, and adds:

- `definition`: the full owning `AgentiflowDef` normalized JSON serialization, including goals and their targets, pool, budget, scope, coverage, round, trigger and messaging. Optional fields/defaults follow that type. This is not the run-detail DTO: objective whitespace is retained and fields are not omitted or expanded from profiles.
- `definition_schema`: exactly `agentiflow.def.v1`.
- `source_sha256`: lowercase SHA256 of the exact submitted UTF-8 `raw` bytes.
- `scope_id`: exact requested registered workspace ID, or null when omitted/null.

Integers remain JSON integers through Rust's u64 serializer and serde_json Number storage. Clients must decode raw response bytes with integer-preserving types; a generic Double conversion is not an exact semantic comparison. Budget tokens and goal count_gte can exceed 2^53. The normalized definition is semantic validation evidence, not a source formatter or persistence receipt. The client retains its submitted source and captured connection/scope/generation, requires the echoed source hash and scope to match, and validates each subsequent proposed edit. Missing capability/echo or refusal keeps new structured controls in Source mode.

## Bounds and refusal

Opt-in source is at most 262144 UTF-8 bytes. The complete serialized response is capped at 1048576 bytes by a bounded writer. The normalized definition is limited to depth 32 (root depth 0) and 16384 value nodes; object keys are not counted as value nodes. Opaque scope YAML values are checked before JSON serialization. Tagged values, non-string map keys and nonfinite numbers refuse rather than becoming lossy JSON. Nonfinite budget/coverage floats are also refused. Returned definition data comes only from the submitted parsed source: there is no credential lookup, substitution or effective profile expansion.

New failures use JSON `{code,error}`: HTTP 413 `definition_source_too_large`; HTTP 422 `definition_echo_unavailable` for JSON representability or echo bounds. Existing parse/profile/check failures retain their current 400 body, unknown scope 404 and remote host 501. Existing unflagged calls retain their original limits and response, including any legacy-valid values the new echo cannot represent.

These bounds cover submitted source and response construction, not a claim of a transactional filesystem/profile snapshot or a total parser/profile-loader RSS budget. Profile validation retains the existing owning loader and its behavior. No new endpoint, write, grant or configuration flag is introduced. Existing operator MCP validation exposes the optional flag through its existing typed request schema; its read-only classification and operation name are unchanged.

## Source validation

Eight fixtures are authored: exact u64/default/whitespace semantics, consecutive whole-definition edits, YAML-only/nonfinite refusal, byte/response bounds, depth/node limits, unchanged legacy/read-only validation, registered scope/profile/local-host checks, and typed opt-in failures. They are unexecuted. Focused validation filters are `rupu-cp --lib validation_echo`, existing `rupu-cp --test it agentiflow_api::validate_runs_the_checks_agentiflow_run_runs`, and existing MCP schema/inventory filters. The source lane performs no build, tests, API/service calls or warm-checkout edits.
