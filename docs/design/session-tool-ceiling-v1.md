# Immutable session tool ceiling

`session.tool_ceiling.v1` adds an optional creation-time upper bound to a persistent session's own model turns. It is independent of `tools`, which remains a declaration and can still receive ambient grants.

## Creation and evidence

`POST /api/projects/{workspace}/sessions` accepts `tool_ceiling: ["skills.load"]`; the CLI equivalent is `rupu session start rupu-session --tool-ceiling '["skills.load"]' ...`. Omitted/null preserves legacy behavior. An explicit empty array denies every tool, including ambient tools. At most 256 unique exact canonical names are accepted. Patterns, aliases, duplicates, malformed names and unknown names are refused before worker launch. Selected MCP identities use their frozen canonical local names, not remote aliases.

The local project-start capability advertises this contract only when the configured starter implements it. Legacy adapters refuse a supplied ceiling; HTTP host forwarding also refuses it rather than dropping it. This change does not add support to the separate legacy agent-start API.

The ceiling is persisted in `SessionRecord.tool_ceiling`. Such records are written at version 7, causing older workers to refuse continuation. Existing records without a ceiling retain their previous written version. Settings cannot add, replace or clear the ceiling. Every normal turn reconstructs the final grant from the persisted value, including after restart and settings changes. This is an application contract, not protection against an operator editing local record files.

`GET /api/sessions/{session}/capabilities` includes the frozen ceiling and the observed assembled-turn snapshot. `effective.tool_ceiling` and each tool's `offered` field describe the actual bounded grant. Snapshot freshness additionally requires the same ceiling. This does not predict later service availability or verify provider credentials.

## Enforcement boundary

The assembler first resolves declared tools, origin/ambient grants and the captured MCP catalog. It then intersects the complete grant with the ceiling; a ceiling never adds a grant or changes its descriptor/body/reasons. The runner creates both provider tool definitions and its sole dispatch registry from those retained entries. A fabricated call to an excluded canonical name or alias therefore follows the existing `not_granted` audit path without invoking the body.

The bound applies to model-visible and model-callable tools in this session's own turns. Explicit operator commands and CP actions remain separate. Configured MCP assembly may still connect before the intersection; an explicit empty MCP selection avoids that discovery/connection work. The bound applies to this session's own tool dispatch. It is not a transitive process-tree sandbox: an allowed spawning or workflow tool retains its normal downstream semantics. A ceiling containing only `skills.load` excludes every spawning, filesystem, messaging, findings, coverage and other ambient tool. The registry and provider tool definitions are fixed for the turn; loading a skill does not modify either. Frozen remote identities remain valid ceiling names after later MCP deselection, but their absence from the resolved grant means no tool is reintroduced. Preloaded skill context and host-side assembly operations remain separate from model-dispatched tools.

## Validation and release ordering

Fixtures cover absent/empty ceilings, ambient exclusion, canonical validation, immutable record reload/settings, real assembler behavior, MCP catalog ownership and fabricated dispatch, capability forwarding refusal, readiness and argv round trips. They are authored but not executed in this source-only candidate. Root schedules executable validation in the shared backend build lane.

This unshipped candidate assigns version 7 to the ceiling. The separately prepared exact-global Conversation candidate must be explicitly rebased to version 8 on top of it and preserve both fields; its original version-7 patch must not be applied as-is. No deployed migration from that unshipped format is claimed.
