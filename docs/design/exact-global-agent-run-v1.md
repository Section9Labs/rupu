# Exact installed global agent run

`agent.exact_global_run.v1` is advertised only on Unix CPs whose actual local
AgentLauncher supports the exact adapter. It does not enable persistent named
sessions, remote launch, arbitrary paths, or runtime/operator MCP tools.

GET `/api/agent-files/run-review?host=local&file_key=installed.md&ws_id=ws_selected`
reads one installed file below the owned global `agents` directory and one exact
registered execution project. It returns schema, host_id=local,
definition_scope=global, binding, definition_sha256, raw, provider/model/description
(nullable recorded agent fields), and source_semantics. The binding is:

- file_key (plain ASCII alphanumeric/underscore/hyphen stem plus .md)
- definition_revision (opaque 64-hex fingerprint)
- effective_name (parsed frontmatter name, separately from filename)
- workspace_id and canonical project_path
- project_revision (opaque fingerprint of exact registration and root/layer identities)

POST `/api/agent-files/run` carries `binding`, `prompt`, optional `host=local`,
mode (`readonly` default or `bypass`), model/provider, engagement_profiles,
findings_profile, skills/preload_skills and existing messaging selection. Unknown
fields are rejected. There is no caller-supplied cwd, target, continuation, session
identity, remote host or arbitrary source body. A successful HTTP200 receipt
contains schema, host_id, definition_scope, binding and new run_id. It confirms
child dispatch, not execution completion. There is one POST and no automatic retry.

CP validates the exact bytes/registration before dispatch; the actual CLI adapter
checks again before spawning; the child checks again before provider preparation.
A changed source or registered root fails closed, never selecting a same-named
project/global file. The CLI records the compact binding in RunStart.launch_args,
separately from effective agent name and execution workspace. CLI continuation
restores that proof and must still pass it; it cannot silently drop stale pinning.

The selected AgentSpec parser reads only Markdown bytes; it has no source-relative
include/path semantics. Bytes are parsed in place, never rewritten or staged under
a different basename. The execution project's existing config/skill and child
catalog layers still apply. Exact root plumbing supports absent .rupu using global
defaults and avoids parent project discovery. Review pins .rupu identity or absence;
config, skills and child catalogs are not snapshotted by this contract. Their normal
permission, grant and resolution rules remain unchanged.

Source <=128KiB and registration <=64KiB. Directory components are anchored without
symlink traversal after resolving the trusted global root. Files must be regular,
single-link, bounded and stable across descriptor/path rechecks. Revisions include
bytes plus file/root identities. These are observed revision checks, not a filesystem
transaction: concurrent changes after the final observation can still occur, and
post-spawn refusal has no synchronous receipt beyond dispatch. No new deadline/RSS
or execution-completion guarantee is made.

Unsupported real adapter is501; wrong host400; malformed input400; absent or changed
source/project409 during CP preflight (no dispatch). Adapter preflight failure400;
spawn failure500 remains uncertain. No auth bypass: existing middleware still owns
HTTP auth and CP write availability is the installed real launcher. Legacy routes
and project-scoped native Run/Conversation remain unchanged. The two new routes
are intentionally HTTP-only in operator MCP inventory: a session tool cannot use
this endpoint to grant itself launch capability.

Source fixtures (unrun at authoring): exact vs shadow/duplicate, absent project
layer and layer appearance, source/registry/root drift, traversal, special files,
reserved/invalid/oversized source, CP read-only review/no dispatch on drift,
unsupported adapter, strict field rejection, actual CLI argv proof restoration and
spawn specification. Provider execution and native acceptance remain separate.
