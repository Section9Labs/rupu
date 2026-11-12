# Session-bound workflow draft launch v1

A saved workflow's local edits can be previewed and run without changing its saved definition. The existing raw YAML launch remains backward compatible. This explicit opt-in path guards the saved baseline and records the actual immutable draft hash separately.

Feature: `workflow.draft_session_launch.v1`, advertised only when the owning control plane has a launcher that supports durable session workflow links. The read-only server has no launcher and does not advertise launching. Preview itself is read-only and may validate on such a server.

## Request

`POST /api/workflows/draft/preview` and `POST /api/workflows/run` accept this exact draft binding in the existing YAML body:

```json
{
  "yaml": "name: example\nsteps: []\n",
  "host": "local",
  "inputs": {},
  "mode": "readonly",
  "engagement_profiles": [],
  "draft": {
    "workspace_id": "ws_example",
    "session_id": "ses_example",
    "source": {
      "name": "example",
      "scope_kind": "project",
      "scope_id": "ws_example",
      "revision": "sha256:<saved baseline bytes>"
    },
    "draft_revision": "sha256:<exact supplied YAML bytes>"
  }
}
```

Values here describe the shape, not live identities. Global source uses `scope_kind:global` and null/absent `scope_id`. A project source must match the execution workspace. The workspace must be registered, its canonical directory must match the active session, and the saved source must exist in the exact selected scope with the supplied baseline revision. Both saved and draft workflow names must match `source.name`. Supplied YAML and the bounded one-FD saved-source read each permit at most256KiB. Baseline revision and draft revision use existing SHA-256 lowercase hex with the `sha256:` prefix, computed from exact UTF-8 bytes.

`working_dir` is refused: the exact registered workspace supplies it. Remote hosts and placed workflow steps (`host`/`distribute`) are unsupported. Omitted mode means readonly; only readonly and explicit bypass are accepted. Ask is refused for all draft sessions and never substituted. Engagement profiles use the existing exact input syntax/parser; this does not freeze an external catalog or resolve agents/providers during preview.

## Preview and launch

Preview returns `{host_id,workspace_id,session_id,source,workflow}`. `workflow` is the actual parsed projection. `source` contains `scope_kind`, `scope_id`, the saved baseline `revision`, and actual `draft_revision`. Preview creates no lock file, source snapshot, session association or run. It is not a frozen authorization; launch repeats every check.

Launch pins supplied bytes in a unique0600 source file, synchronizes file and directory, then uses the existing durable session association/reserved run identity before invoking the launcher. The saved-definition writer lock spans the launch-time baseline check through source pinning; preview never holds that lock. The existing session launch boundary then revalidates the session. This does not claim that later external file edits are locked or that execution reads the saved definition again. The saved definition is never modified. The source snapshot remains available after spawn failure.

The launch response contains `{run_id,host_id,workspace_id,session_id,source,association,file}`. Its `source` equals the association's source. `source.revision` is explicitly the saved baseline, **not the executed draft**. `source.draft_revision` identifies actual executed bytes. Existing named launch associations omit this optional field; persisted old associations remain decodable. Clients must preserve and label the distinction when reopening session history.

A failed launch response can occur after the association was durably reserved or a process was attempted. Clients must preserve their pre-dispatch immutable intent and unknown outcome; HTTP status alone is not proof of zero effects. No automatic retry, fallback to unbound YAML launch, or fallback after saved-launch refusal is authorized. This endpoint has no idempotency token or operation lookup beyond existing session workflow records; matching a later run by name/hash alone is not proof of a particular attempted request.

## HTTP and MCP boundary

Preview is explicitly HTTP-only in route inventory and MCP exclusions. No new MCP tool or grant is added. Existing `workflow.run_yaml` remains present; the additive draft field is omitted from its published JSON schema. Existing unbound YAML launch behavior is unchanged.

## Source-only qualification

Ten prepared fixture declarations cover read-only preview/no lock file, actual source/projection, source pinning and default mode, stale/foreign/missing ownership/hash/name/mode rejection, session project mismatch, both source byte bounds, readonly launch/capability gating, durable spawn-failure evidence, global source execution scope, legacy source decoding, and explicit host/distribute refusal before preview or source pinning. They have not been executed in this lane. No build, live API call, service, GUI, remote publication or real launch was performed.

Later validation: `cargo test -p rupu-cp --lib draft_`, `cargo test -p rupu-cp --lib session_workflow`, the existing workflow tests and `cargo test -p rupu-cp --test it mcp_routes::`, using the integration owner's normal feature configuration. Preserve the pinned MCP tool names. Native acceptance must additionally verify an unchanged saved source, actual session association, exact draft source hash and no duplicate submission after an uncertain response.
