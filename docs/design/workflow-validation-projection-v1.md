# Workflow validation projection v1

This additive HTTP authoring API parses caller-supplied bytes. It does not read or write definitions, resolve agents, providers or credentials, consult project stores, or prepare execution. Existing `POST /api/workflows/validate` remains unchanged. The feature is advertised in `GET /api/host/info` as `workflow.validation_projection.v1`, including when execution adapters are unavailable. Existing authentication middleware applies unchanged. The route is explicitly excluded from the operator MCP inventory; this does not grant a model a new tool.

## Request and response

`POST /api/workflows/validate/projection`, JSON with exactly:

```json
{"schema_version":1,"nonce":"draft_1-Nonce","raw":"name: demo\nsteps:\n  - id: one\n    agent: review\n    prompt: hi\n"}
```

Nonce must be 1–128 ASCII letters, digits, underscores or hyphens. Unknown fields and unsupported versions are refused. Success contains exactly these eight fields:

```json
{
  "schema":"workflow.validation_projection.v1",
  "schema_version":1,
  "parser_contract":"rupu.workflow.v1",
  "nonce":"draft_1-Nonce",
  "source_sha256":"lowercase SHA256 hex of exact raw UTF8 bytes",
  "source_bytes":78,
  "ok":true,
  "workflow":{"name":"demo","steps":[]}
}
```

The workflow above is schematic, not a valid response fixture. Actual `workflow` is the complete existing `Workflow` serialization after the owning `Workflow::parse` succeeds, with all fields/defaults retained. It is never a page or truncated subset. The source hash and size use exact decoded `raw` UTF8 bytes, including CRLF, Unicode spelling and final newline. They are not hashes of normalized YAML or of the outer JSON request. `workflow.name` is the parsed name; there is no independent name echo.

There is no project, definition revision, saved-source freshness, runtime readiness, ownership, Save or launch claim. The caller must bind the result to its captured endpoint/profile/document/project, source, request nonce and draft generation, and refuse stale results. A validation result cannot clear a Save conflict or unknown outcome. Clients feature-gate this endpoint and use Source fallback when unavailable; they must not synthesize validation evidence.

## Bounds and exactness

- HTTP decoded body: 2 MiB, including JSON escaping; decoded raw source: 256 KiB.
- Before typed workflow materialization: a non-materializing YAML visitor limits expanded depth to 64, total visited nodes to 32,768, each collection to 2,048 entries, and cumulative expanded string scalar bytes to 1 MiB. Alias expansion is charged each visit. Source/event scanning is bounded by source bytes and the existing YAML parser's limits.
- Two blocking parser jobs process source at a time. Admission is immediate: no application queue. The actual blocking work owns its semaphore permit across HTTP waiter cancellation; a disconnect does not release capacity early. These are finite work bounds, not a measured latency or hard wall-clock guarantee.
- Complete serialized response: 2 MiB, depth 64, 65,536 value nodes including envelope. The larger response node bound allows serializer defaults/wrappers. The response does not separately promise raw-YAML scalar or collection bounds. An over-bound response is discarded in full.
- Existing integer tokens, including `u64` input defaults greater than 2^53, retain their exact JSON integer representation. A client using floating-point-only JSON storage must reject unsafe integers and keep Source available, rather than round them. This API does not constrain the owning workflow schema to floating-point precision.
- Custom YAML tags, non-string mapping keys and non-finite numbers are unavailable for this JSON projection, even if another parser path could accept them. Standard scalar tags such as `!!str`, `!!int` and `!!bool` are handled by the owning YAML parser as ordinary typed scalars. Mapping semantics, defaults and validation remain those of the existing parser; this is not a YAML text round-trip. Structured editors must retain independent source-span eligibility checks and must not re-emit source from this JSON.

## Errors

All errors contain fixed `code` and `error` strings without source text, parser diagnostic excerpts or unknown-field names. No source payload logging is added.

| HTTP | Code | Meaning |
|---|---|---|
| 400 | `workflow_invalid` | Malformed JSON/YAML, invalid request/schema/nonce or invalid workflow |
| 413 | `projection_limit` | Request, source, expanded work or complete response exceeds a limit |
| 422 | `projection_unavailable` | Unsupported exact JSON representation |
| 429 | `projection_busy` | Both parser jobs are occupied; no work admitted |
| 500 | `projection_failed` | Blocking work could not complete |

The operation has no effects to retry or reconcile, but clients should present an explicit Validate/Apply action rather than silently loop on busy responses. No source is persisted by this route.

## Qualification

Thirteen authored fixtures cover exact UTF8 source binding and owning serialization, large integer defaults, 1,025 complete steps, strict envelope, invalid/unsupported YAML, bounded expansion/response, sanitized errors, detached-work permit lifetime, stateless HTTP/capability with absent stores/adapters, unchanged legacy validation, and oversized/unknown HTTP envelopes. They have not been compiled or executed in this source-only slice. Rust formatting/parsing and diff whitespace checks are not runtime or performance evidence. Later integration must run the `workflow_validation_projection` test filter and the MCP route inventory suite, then exercise the native shared-draft journey with a tested matching backend.
