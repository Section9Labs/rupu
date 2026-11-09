# Explicit query syntax for retained workflow history

Base: `365bb3b09a8e9478d8daeb4c545c6a2dd91c1967`.

`workflow.session_history.query.v1` advertises explicit syntax on the existing read-only `GET /api/projects/:id/workflow-history` route and `projects.workflow_history` MCP tool. It is available without a launcher. No route/tool inventory entry is added.

Omitting `query_mode`, or sending `query_mode=literal`, retains the existing case-insensitive trimmed substring matching across workflow name, run ID and codename. Colons and quotes remain literal. No syntax is inferred. The literal response envelope is unchanged.

`query_mode=syntax` opts into the existing `workflow_runs` field registry and parser. Fields are `status`, `workflow`, `id`, `codename`, `trigger`, `host`, `customer`, `since`, `until`, and `is`. Quoting, negation, value lists, matching modes and time interpretation are exactly those of that shared registry. `host` is always `local`; `is:archived` reads the retained archive flag. Customer predicates use recorded attribution, falling back to the current project assignment for legacy records just as project-runs queries do; unreadable assignment fails closed instead of becoming `customer:none`.

Syntax `q` is at most 4096 UTF-8 bytes and 128 parsed terms; exceeding either returns 413 `{error}`. Invalid grammar returns the existing structured 400 `{error,token,code,start,end}` with Unicode-scalar offsets. Invalid query modes and invalid page limits return 400. Empty syntax queries match the owned catalog and echo an empty `q`.

Responses retain `{workspace_id,host_id,runs,next_offset}` and syntax responses additionally echo `query_mode:"syntax"` and the exact supplied `q` string (not trimmed or normalized). The existing row fields and canonical `run_id` remain unchanged. Native clients must validate these echoes and keep literal search on servers lacking the feature. Shared parser check success only validates grammar, not query limits, execution or matching results.

The registered project is required (unknown projects return 404), and transport remains local-only. Active records win over archived copies of the same ID **before** project/query filtering. Worker-tagged mirrored records are excluded in both modes so they cannot be mislabeled local. Filtering precedes offset pagination (limit 1–100, default 50); sorting remains newest-first with run ID tie-breaking. A syntax query cannot widen the project/host scope, restore an archived run, launch execution, or change source/transcripts.

This is not a captured search snapshot. The underlying active/archive catalog enumeration is unchanged and is **not bounded by the page size**. Bounds here cover the query and returned row count, not total source scanning. Repeated offset pages can observe intervening catalog changes. Customer attribution adds at most one memoized assignment lookup per owning project per request; transcript, event and usage histories are not scanned.

Validation plan: focused `workflow_history_query_` unit fixtures, existing `session_workflow_history_` archive/ownership regression fixtures, and `workflow_history_syntax_tool_matches_http_and_exposes_explicit_mode` operator parity fixture. No build or runtime request was performed in the implementation checkout.
