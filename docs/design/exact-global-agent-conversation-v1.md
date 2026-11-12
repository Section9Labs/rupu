# Exact saved global agent conversation

`agent.exact_global_conversation.v1` is advertised only on Unix when the installed
local SessionStarter explicitly supports this contract. This is a persistent
conversation, separate from `agent.exact_global_run.v1` and detached Run.

GET `/api/agent-files/conversation-review?host=local&file_key=chosen.md&ws_id=ws_chosen`
uses the existing exact global definition reader. It reads one installed global
file and one registered project, with the same bounded, no-follow file proofs as
exact global Run. The reply has `schema`, `host_id: local`, `definition_scope:
global`, `binding`, `definition_sha256`, `raw`, optional provider/model/description,
`supported_modes: [readonly,bypass]`, an explicit Ask-unavailable reason and source
semantics. Binding fields are the existing exact Run fields: file_key,
definition_revision, effective_name, workspace_id, canonical project_path and
project_revision. A same-named project file cannot substitute for the global file.

POST `/api/agent-files/conversation` accepts that exact `binding`, a nonempty first
`prompt` (at most32768 UTF-8 bytes), optional host=local, mode (readonly default),
provider/model, tools, skills/preload_skills, mcp_tools/mcp_catalog_revision. Unknown
fields are refused. No cwd, target, scope override, raw agent body, arbitrary path,
idle creation, session continuation or launch-link override is accepted. Provider
override requires model. Tools omitted inherits the selected agent, [] removes its
declared tools, and a custom list replaces them; existing ambient/origin rules
remain. MCP omitted retains existing configured defaults; [] disables the session's
external selection; explicit nonempty selections retain discovery revision checks.
This route does not enable MCP servers or grant operator MCP access.

Skills remain selected named-agent skills plus explicit additive selections and
preloads. The builtin `rupu-session-full-v1` preset is specific to the builtin
harness; it is neither applied nor silently ignored here. A skill_preset field is
rejected. Ask is also rejected rather than downgraded: the current subprocess
starter permits the private authenticated Ask channel only for builtin
rupu-session, and named detached sessions do not have that channel. The review
response makes the supported modes explicit. This slice does not broaden Ask.

Successful HTTP200 receipt returns schema, host_id, definition_scope, exact
binding, session_id and `status_semantics: session_created_first_turn_submitted;
not_completion`. No run_id is substituted. CP checks definition/project revision
before invoking the supporting adapter. The adapter checks again before process
creation; the child checks before assembly and again before persistence. It uses
the captured AgentSpec and exact workspace registration, never project-shadowed
name lookup or path-deduplicating workspace upsert. The existing config, skills,
child-agent catalogs, credentials and tool policy still resolve for the chosen
execution project. Review does not snapshot those dependencies or contact a
provider. Normal first-turn preparation retains its existing SCM discovery/MCP
selection behavior; no acceptance test may claim it is generally network-free.

Persistent records carry `exact_global_agent` with encoded binding and the frozen
agent fallback chain, in addition to existing frozen agent fields. Record version8
protects this provenance from older writers. Later turns use the frozen fallback
chain; deleting/editing the original definition or a project shadow cannot make
an existing exact conversation change its selected agent. Existing session
settings and recovery-config fallback behavior when no agent chain was declared
remain. Persisted agent/workspace/project identities are checked before turn
assembly. Ordinary sessions retain their previous written versions and legacy
fallback resolution; version8 is written only for exact conversations. Old binaries
may display newer records through their existing read-only path, but cannot send or
rewrite them. Delivery must include a compatible backend; downgrade must not erase
or reinterpret the provenance field.

There is one submission, no automatic retry, no idempotency key, and no server
receipt lookup added by this slice. CP/pre-spawn malformed input is400, unsupported
starter501, and observed selection drift409. Once the subprocess has started,
nonzero exit or missing receipt is500/uncertain even if a session may already have
been persisted. Client cancellation/transport loss is likewise uncertain. Native
must persist the exact profile/endpoint/project/binding/options/first prompt before
POST, preserve the intent/draft on uncertain outcome and block blind resubmission;
a fresh review alone is not proof that a prior creation failed. Successful receipt
must match the captured owner/binding and open the returned session without running
it again. Lost receipt reconciliation is manual read-only session inspection in
this version, not an automatic replay. Observation checks are not a filesystem
transaction: owner/source changes after the last check are not atomic-CAS covered.

Both routes are HTTP-only operator actions in MCP route inventory/exclusions. No
new model tool, remote proxy, service, provider request, runtime credential change,
or Agentiflow resume is introduced by the implementation work.

## Source validation plan

Thirteen authored fixtures are source-only and unrun at handoff:

- CP6: review/receipt owner+kind and zero review dispatch; source/registration drift;
  remote/missing adapter/Ask/unknown-field refusal; first prompt/provider pair;
  one-shot uncertain result; omitted/empty/custom declarations.
- CLI preparation5: global-vs-project shadow and fallback persistence; stale/wrong
  selection; target/idle/Ask/empty prompt; persisted owner mismatch/legacy/version
  refusal; registered project without local layer ignoring parent definition.
- CLI adapter2: exact proof/argv; request-owner rebinding refusal.

Run in the coordinated warm slot with default features and jobs2:

```
cargo test -p rupu-cp --lib api::agents::exact_session::tests:: -- --test-threads=2
cargo test -p rupu-cli --lib session_exact_agent::tests:: -- --test-threads=2
cargo test -p rupu-cli --lib cp_session_starter:: -- --test-threads=2
cargo test -p rupu-cli --lib session_harness_tests:: -- --test-threads=2
cargo test -p rupu-cli --lib session_skills -- --test-threads=2
cargo test -p rupu-cp --lib api::agents::tests:: -- --test-threads=2
cargo test -p rupu-cp --lib api::host_info:: -- --test-threads=2
cargo test -p rupu-cp --test it mcp_routes:: -- --test-threads=2
```

The last checks retain legacy scope and registry behavior. Runtime/network/GUI
acceptance remains separate. Authoring performs only parsing/diff and exact export
checks, not compilation or provider execution.

## Version 8 integration with immutable tool ceilings

Exact conversations write record version 8, including when no tool ceiling is selected. Ceiling-only sessions remain version 7; ordinary sessions without either field retain their prior minimum written version. Both optional fields are serialized together and survive the existing whole-record clone, turn save and archive paths. Exact creation accepts the same optional canonical `tool_ceiling` as project sessions; omitted/null preserves legacy grants, while `[]` denies all model tools. Unsupported starters and invalid/unknown/wildcard names refuse before dispatch. Selection and settings never widen the immutable ceiling. The existing per-turn assembler still intersects the final declared/ambient/MCP grant and freezes registry/schema advertisement and dispatch. No descendant ceiling or OS sandbox is implied.

This is an isolated source-only rebase over V7, not part of its frozen release. New version-8 binaries must be built and focused tests run before enabling the exact Conversation capability.
