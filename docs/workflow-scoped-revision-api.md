# Scoped workflow definitions and guarded launches

HTTP control planes advertise `workflow.scoped_revision` in `/api/host/info`.
Clients must require this feature before relying on scope or revision guards;
older endpoints may silently ignore unknown query/body fields.

## Definition identity

`GET /api/projects/:workspace_id/workflows` lists the effective global and project
catalog. Project definitions shadow global definitions by name. Identify a row
by endpoint, name, `scope_kind` and `scope_id`, never its display `scope` label.

`GET /api/workflows/:name?scope_kind=project&scope_id=:workspace_id` strictly
selects that project definition. `scope_kind=global` selects the global definition
and takes no `scope_id`. Missing selected definitions return 404 without fallback.
An omitted selector retains the legacy project-first resolution.

Detail returns the existing `workflow`, `yaml`, `usage`, `scope`, `scope_kind`, and
`scope_id`, plus opaque `revision` (`sha256:` followed by the raw UTF-8 YAML hash).
Invalid YAML still fails parsing on detail; this is not a raw-file recovery API.
Usage remains attributed by workflow name and can combine definition scopes.

## Create, validate and save

- `POST /api/workflows` with `{raw, scope_kind:"project", scope_id:"ws_…"}`
  creates a definition only in the registered project's `.rupu/workflows`.
  An existing same-layer name is 409; explicitly shadowing a global name is allowed.
  Without project scope, legacy global creation and collision behavior remain.
- `POST /api/workflows/validate` with `{raw}` returns `{ok:true}` or 400. This
  checks the workflow parser, not agent availability, provider readiness, inputs,
  permissions, or successful execution.
- `PUT /api/workflows/:name?scope_kind=…&scope_id=…` with
  `{raw, expected_revision:"sha256:…"}` checks the selected existing definition.
  Stale bytes return 409 before write. A missing scope/definition is refused;
  scope is mandatory when using the revision guard. The response reloads the
  exact selected layer, including its new revision. Omit `expected_revision`
  only when intentionally requesting legacy unconditional save behavior.

CP workflow create/save/delete and autoflow enable/disable cooperate through a
per-directory advisory writer lock across processes. External editors and other
noncooperating writers do not use that lock; this does not provide universal
filesystem compare-and-swap. Revisions detect changes present at the guarded read.

## Guarded launch

`POST /api/workflows/:name/run` accepts:

```json
{
  "scope_kind": "global",
  "expected_revision": "sha256:…",
  "workspace_id": "ws_…",
  "host": "local",
  "mode": "readonly",
  "inputs": {"subject": "Review the actual project changes"}
}
```

For project-owned definitions, also send `scope_id`; when `workspace_id` is
provided it must equal that owner. Global definitions can execute in a different
registered project via `workspace_id`. An unknown execution workspace is 404.
`workspace_id` requires a revision guard and explicit source scope, and cannot be
combined with `working_dir`. Omitted `workspace_id` preserves scope-derived cwd.
Unqualified legacy launches retain their existing behavior.

A stale revision returns 409 before any source snapshot or launch. A successful
guard validates and writes the exact selected YAML to a unique durable file under
`<global>/cp/workflow-launches`, using owner-only permissions on Unix, then launches
with `--file` and the selected execution cwd. The snapshot is not temporary and
is not removed when the detached launcher returns. It remains available for the
child and recorded provenance; no new automatic retention policy is introduced.

The response is `{run_id, host_id, source:{scope_kind,scope_id,revision},workspace_id}`.
It is a launch receipt, not proof that steps completed. Guarded remote-host proxy
launches return 501; connect directly to the owning HTTP control plane instead.

The CLI resolves project config, named agents and relative execution resources
from the execution cwd, not the YAML file's parent. Pinning the workflow does not
pin agent definitions, model/provider configuration, external files, or other
runtime dependencies. The workflow parser has no source-file-relative import
mechanism. Run-step `cwd` retains its existing workspace-root default.
