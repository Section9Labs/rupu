# Owner-qualified project fleet unavailability

GET `/api/fleet?ws_id=<registered ID>&host=local` retains its successful response shape. Project-layer conflicts now return HTTP 409 with an additive structured reason:

```json
{
  "error": "The registered project has no project-local .rupu directory.",
  "schema_version": 1,
  "code": "project_layer_missing",
  "scope": {
    "host_id": "local",
    "workspace_id": "ws_example",
    "project_path": "/registered/project"
  },
  "project_root": "/registered/project/.rupu",
  "consistency": "observed_not_locked"
}
```

Codes are `project_layer_missing`, `project_layer_global_alias` and `project_layer_unavailable`. Only `missing` positively identifies a missing final `.rupu` component distinct from the configured global root. The reader canonicalizes the existing project and global anchors and inspects the final component without following it first. A global alias, dangling symlink, non-directory layer, inaccessible/unresolvable anchor or unknown error must not be treated as missing. Unknown registered IDs remain 404; remote-host queries remain 501. Global-only fleet reads and successful project inventory retain their original wire shape. No new endpoint, MCP tool or advertisement is added.

Consumers must validate version, exact local host/workspace/registered path, project root and consistency before using the missing reason to enable initialization. Legacy unstructured 409 and unknown codes must leave initialization disabled. A native client should repeat the same read immediately before its separately reviewed init request; there is no client-side inference from lexical global/project path strings.

This is a read-only observation, not an init reservation, registry CAS or hostile-filesystem race guarantee. The existing init/install mutation routes and authorization gates are unchanged. A registry or filesystem change after observation remains possible and must not be described as atomically locked. Native request ownership preflight and returned-root checks remain advisory.

Seven source fixtures cover exact missing evidence/no creation, global alias without configuration, symlink aliases, dangling symlink, non-directory/removed project, unchanged successful inventory/global response, and existing unknown/remote errors. Focused validation filter: `cargo test -p rupu-cp --lib api::fleet::project_layer_tests`. Existing integration regression filter: `setup_api` (especially fleet/project-init cases). This isolated task ran rustfmt and patch checks only; builds/tests and real runtime acceptance belong to the scheduled validator. No real project/global initialization or fleet installation was performed.
