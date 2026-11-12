# Scoped configuration revisions

`config.scoped_revision.v1` in `GET /api/host/info` identifies the versioned selected-layer editor. Existing `/api/config` views and global writes retain their contracts. This feature is HTTP-only; existing MCP configuration tools retain their legacy schemas.

## Wire contract

- `GET /api/config/scoped/project/:id`
- `PUT /api/config/scoped/project/:id`
- `GET /api/config/scoped/customer/:slug`
- `PUT /api/config/scoped/customer/:slug`

The only optional query field is `host=local`. Remote hosts return 501. Authentication is the CP router's existing authentication. PUT also requires the existing writable CP marker; a read-only deployment returns 501.

GET returns:

```json
{
  "scope": {"kind": "project", "id": "ws_example", "host_id": "local"},
  "revision": "sha256:<64 lowercase hexadecimal digits>",
  "raw": "default_model = 'example'\n",
  "exists": true,
  "customer_slug": null,
  "revision_scope": "selected_and_inherited_config_with_resolved_owner",
  "registry_consistency": "rechecked_not_locked"
}
```

`kind` is `project` or `customer`; `id` is the exact route identifier. `raw` is only that selected layer, never effective/inherited TOML. `exists` distinguishes a missing file from an empty file. A customer response's `customer_slug` is its own slug; a project's is its currently resolved customer or null. Clients must compare the exact scope with their captured owner before adopting results.

PUT requires `expected_revision` and exactly one of `raw` (string) or `patch` (the existing flat form-patch object). Missing revision is a decode error; malformed revision or ambiguous edit is 400. A stale revision returns 409 without changing config bytes. Success returns the same selected-layer fields plus `ok: true`. The returned revision describes the committed bytes and the dependencies validated for that write; it is not an unlocked follow-up read.

The opaque revision binds the scope, selected path, canonical project path, resolved customer association/path, and exact selected/global/customer configuration contents, including missing versus empty. Changing inherited configuration, moving the project, or changing the resolved customer invalidates a previously observed revision even if the selected TOML did not change. Captured layer content and candidates are limited to 1 MiB. The existing resolver validates those files while cooperating config writers are locked out; noncooperating manual writers remain outside that guarantee. GET performs two equal bounded captures and creates no config files, directories or lock sidecars. It can serve malformed selected TOML for repair; non-UTF-8 or oversized layers fail explicitly.

## Transaction boundary

All scoped PUTs, including legacy project/customer PUTs, acquire config sidecar locks in global → customer → project order. The selected file is read, the expected revision compared, a patch materialized, inherited policy checked, the merged candidate validated and the file atomically replaced while those locks are held. Existing atomic backup/replacement behavior is reused. Legacy scoped PUT responses remain `{ "ok": true }`; they do not claim stale-editor protection, but concurrent patches now merge against the locked current file. Legacy global/policy handlers are unchanged.

Inherited policy is still enforced by the runtime resolver. Write validation preserves existing global/customer policy refusals and additionally checks current disk policy and merged provider dependencies while holding the relevant config locks. No provider call, session launch, service restart or model execution occurs.

Workspace/customer registry and assignment writers do not participate in these locks. Resolved identity/path/association is captured in the revision and rechecked immediately before replacement, but this is **not an atomic registry or filesystem snapshot**. A concurrent registry/assignment change after that final check can leave a successful receipt referring to the previous observed association, or write the prior canonical project path just as the project is relocated. The exact file selected and validated is replaced; a later GET obtains the new owner/dependencies and revision. Runtime policy resolution continues to enforce the then-current inherited locks. Noncooperating manual file writers and hostile filesystem rename/symlink races are also outside advisory config-file CAS.

Clients must preserve a submitted edit and its original owner when a connection changes. An absent/invalid receipt or transport failure after PUT is an unknown outcome: do not retry automatically. Read the same exact scope and compare saved source to the immutable submitted source before offering reconciliation; a newer local draft is separate evidence. This backend adds no retry behavior.

## Prepared validation

Source-only at export: no builds or services launched. The focused test module covers exact/missing/empty scope reads without filesystem writes, mandatory revision and raw/patch receipts, stale/foreign revisions, simultaneous guarded writers, simultaneous legacy patches, global/customer/assignment dependency changes, project relocation, policy/provider rejection, malformed-source repair, bounded reads, unsupported writes, and unchanged global response shape. The MCP route inventory explicitly excludes all four new HTTP routes. Run the existing config API tests and route inventory tests alongside the focused module in the warm validation checkout before release.
