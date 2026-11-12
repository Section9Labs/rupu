# Scoped Rupu reads for builtin sessions

This source candidate adds four builtin Read bindings to newly created `rupu-session` defaults. It does not configure external MCP, connect the operator MCP endpoint, add credentials, change permission mode, or grant tools to existing sessions through inherited wildcards. Build, test execution, actual providers and native acceptance remain separate integration gates.

## Default and wire contract

- New default declaration provenance: `rupu-session-tools-v2` (`rupu_agent::builtin::SESSION_TOOLS_PRESET`). The exact declaration is returned by project capability planning; clients must not hardcode a count or construct `*`.
- New host feature: `session.scoped_reads.v1`, advertised only by a builtin-capable starter that implements the worker service. It is independent of `session.mcp_tools.v1` and skill presets.
- Project and session capability responses add `builtin_tools_preset: String|null` and `scoped_reads_version: integer|null`. The latter records supported reader semantics, never a permission override. Project null means starter support is not established. Session value records **creation-time default provenance**, not current effective grants. A later settings override can be empty while this historical provenance remains v2; render `declared_tools`/effective evidence as authoritative for current selection.
- SessionRecord adds optional `builtin_tools_preset` and `scoped_reads_version`; its minimum write version is 6 when either is present. New builtin sessions carry reader version 1 even with custom/empty declarations; only unmodified defaults carry the preset marker. Old records retain their frozen declarations and no markers. Inherited legacy `*`, `session.*` and `run.*` do not acquire new reader authority. A legacy session can select exact reader names through a persisted settings update (revision greater than zero); each exact name enables only its own reader, even alongside `*`. Unknown future reader versions fail closed. Explicit empty/custom declarations remain authoritative. Older runtimes must refuse mutation of version 6 records.
- The native UI should group these descriptors under **Rupu tools**, keeping **Configured MCP** and **Skills** separate. Static declarations, service availability, permission and actual assembled evidence are distinct. These tools have no remote identity.

## Four tools

| Name | Arguments | Evidence |
|---|---|---|
| `session.context` | Empty object | Captured current-turn owner, provider/model, permission, tools/skills selection, settings/orchestration revisions; never credentials, prompt or other-session data. Selection is not execution proof. |
| `session.workflows` | Optional bounded offset/limit | This session's stored workflow associations, with recorded status and live-directory pagination qualification; no global/project catalog scan. |
| `run.inspect` | Optional ordered `ancestry` child-ID chain, empty=current turn | Positive recorded child membership and matching workspace/run prologue, compact event counts and child identifiers; no raw reasoning, prompts or tool payloads. |
| `run.netflows` | Same ancestry chain | Owned local artifact endpoint/outcome counts, capture/drop/partial qualifications; no URLs, request bodies, headers, credential values or global fallback. |

All four use `Effect::Read` with separate service capabilities: `SessionContext`, `SessionWorkflows`, `RunInspection` and `RunNetflows`. The worker computes exact allowed readers from creation provenance and current selection; service exposure, body construction and invocation all enforce that set. Service construction adds no grant: bodies are offered only when existing grant resolution permits the declared names. The service is bound to the exact root session run/workspace/path, checks the live ToolContext at each call, and does not propagate through child launch services. A root can inspect a child only through a recorded ancestry chain. Child tools cannot borrow the root service.

Readers use bounded files/lines/rows/output and descriptor-relative no-follow regular-file reads. Trusted root aliases are mapped to pinned canonical roots without opening future transcripts; untrusted aliases and parent traversal are rejected. Netflow rows missing legacy owner annotations are explicitly counted as artifact-path-only evidence and mark the result partial. Malformed dropped counts fail closed rather than becoming zero. Missing, malformed, oversized or changed ownership fails closed or returns explicit partial/unavailable evidence as appropriate. Partial prefix counts are not full execution status or complete network capture. The service never reads provider/MCP credential stores or invokes an HTTP operator route.

## Mapping and intentional limits

`session-scoped-tool-mapping-v1.json` inventories static operator specs in the exact source base, with source pointers. Its classes distinguish existing same-name runtime tools (different scope), new scoped projections, and operator-only operations not delegated. It is not installed MCP enumeration: dynamic route expansion, wait and optional catalog composition are separate.

Existing builtin catalog, dispatch/join, coverage, findings, assets, SCM and issue tools retain their own service and permission behavior. Readonly still denies Write/External and Spawn still carries its ceiling. No operator route is granted merely because it is GET or carries readOnlyHint. MCP registry/credentials, self-grant settings, global administration and destructive operations remain outside this capability set.

Ask approval implementation is a separate branch/contract. This slice neither removes strict Ask rejection nor changes native permission defaults.

## Verification prepared

- Runtime reader fixtures cover exact owner/ancestry, bounded observations, malformed identity, filesystem substitution/special files and output privacy.
- CLI fixtures cover new default provenance roundtrip, legacy freeze, explicit empty/custom selection, and actual worker assembly with a mock provider calling `session.context` versus a not-granted attempted call.
- CP fixture covers supported versus unsupported starter metadata, legacy unknown provenance, current explicit empty settings overriding a stored default, and read-only capability inspection.

No test result is claimed until the root's isolated validation executes these fixtures. Actual OpenAI/Anthropic acceptance must separately show the exact first-turn assembled builtin grants and successful scoped results in a disposable project. The retained MCP fixture and its mock provider do not satisfy that requirement.
