# Exact agent diagnostic source repair v1

Approved scope: additive local agent-file read and revision-bound replacement, exact a42e3bea base. Feature `agent.source.repair.v1` on Unix. Existing agent APIs and runtime authorization remain; no launch, model call, skill grant, rename, create, or alternate layer fallback occurs through this route.

GET and PUT `/api/agent-files/source` use the same query: `host=local` (or omitted), required `scope_kind=global|project`, `scope_id` required only for project, required `path` equal to the exact diagnostic file path. Unknown/duplicate selectors reject. Path is not authority: server derives global agents or exact registered project/.rupu/agents from trusted state, then requires a single existing .md child. It never uses the diagnostic name, first parsed agent match or catalog ordering. The configured/registered lexical path and its canonical equivalent are accepted, but the response always identifies the canonical path. Native retains both requested and canonical paths and uses the canonical path for later writes.

Workspace lookup reads only `workspaces/<safe-id>.toml` (<=64KiB), requires matching record.id and an existing absolute project root, and binds its exact bytes/identity. No full workspace catalog scan. Project aliases of global storage refuse. Anchored descriptor walks reject intermediate/final symlinks and nonregular files. Files must have one link, be valid UTF-8 and <=256KiB. Unsupported file stems remain readable where validate_stem permits them, but save_supported is false if existing authoring name rules cannot save them.

Successful source envelope, all fields present:

```
schema_version: 1
host_id: "local"
owner_instance_id: string
scope_kind: "global" | "project"
scope_id: string | null
requested_path: string
path: string                  # canonical exact selected file
file_stem: string
revision: string              # opaque scoped digest, retain unchanged
raw: string
parsed_name: string | null
parse_error: string | null
authoring_error: string | null
save_supported: bool
consistency: "observed_with_revision_checks"
saved: bool                   # false for GET, true for successful PUT
request_id: string | null
previous_revision: string | null
```

Revision binds canonical owner and directory identity, exact scope/project registration, target file identity and captured bytes. owner_instance_id is a nonsecret owning-global-root digest, bound to the exact client; not a new host-info identifier. GET succeeds for invalid source, name/stem mismatch and shadowed files. Parse error does not hide source. authoring_error follows existing syntax/tool-name/skill-entry validation; missing installed skills remain allowed as before. Neither a successful read nor save asserts runnable/effective permissions.

PUT JSON: `{raw, expected_revision, request_id}`. All required; unknown keys reject. request_id is a caller-generated bounded identifier echoed for acknowledgement, **not an idempotency key**. Native persists exact client/scope/path/revision/proposed-content digest/request ID before dispatch. Repaired text must satisfy existing parse_agent_for_write and frontmatter name==actual file stem. No malformed save, grant escalation or filename change is enabled. Read-only CP may GET; PUT requires existing writable adapter.

Lock order: resolve scope without a lock; acquire ONE per-agents-directory `.agent-definitions.lock` nonblocking; re-resolve owner; read/check revision; validate and stage bytes; recheck scope/revision; replace and sync; return exact selected source. Never acquire a second directory lock or reenter locked helpers. Updated legacy agent HTTP create/PUT/DELETE acquire the same single lock around their writes. Helpers called beneath these handlers do not reacquire it. External editors, older binaries and other lower-level callers do not participate: this is cooperative revision checking, not adversarial-filesystem atomic CAS or a transaction with the workspace registry.

Sibling staging uses exclusive random temporary names, regular nofollow descriptors, existing file mode, fsync then descriptor-relative rename and parent-directory fsync. Revision drift before rename refuses409. An error after rename may mean content changed even if durability/receipt is unknown. No backend operation journal or exactly-once claim is introduced.

Success returns the exact selected file plus request_id/previous_revision, never the legacy project-first detail resolver. On transport loss or 5xx after submitting PUT, retain unknown outcome and exact recovery identity; GET the same source. Matching current bytes are an observation, not proof that a particular request wrote them. Never blindly retry PUT or infer safe replay from a missing response.

Errors use existing `{error:string}`:400 invalid scope/path/authoring/request fields;404 exact file/registration missing;409 owner/registration/file/revision conflict or unsafe source;413 size bound;429 cooperative writer busy;501 remote/unsupported or PUT on read-only CP;500 storage failure (postrename text includes `agent_source_save_unknown`). Extractor errors may use existing framework shapes. Preserve old source/draft on every refusal.

Read/response raw cap256KiB; encoded request/response caps2MiB accommodate JSON escaping; path4096 bytes, file stem128, scope ID128, revision64hex, request ID128 ASCII identifier. One exact registration and one exact agent file are read, with bounded rechecks; no directory enumeration or transcript reads. MCP parity: `agents.source` GET and `agents.repair` PUT with the same explicit selectors, existing operator authorization, and no automatic runtime tool grants.

Required disposable fixtures: invalid/shadowed raw reads; exact global/project sibling isolation; malformed/foreign registry; stale content/file/root/registration revision; symlink and hardlink/special-file refusal; byte caps; authoring refusal; legacy/new lock contention; success exact source; failure before rename unchanged and postrename failure unknown with exact file available for recovery. Builds and runtime acceptance belong to the coordinated queue, never actual-store repair experiments.

GET performs bounded observed identity/revision checks without creating or acquiring a lock file. The common consistency label describes those checks; only writes participate in the cooperative directory lock. MCP repair is marked destructive and non-read-only because it replaces existing source.
