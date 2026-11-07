# Session capabilities v1

The control plane always advertises `session.capabilities.v1` for these read-only,
local-host endpoints. Capability reads do not create sessions, assemble turns,
resolve providers or credentials, install skills, or write settings. Clients
must gate the capability views on this feature; a server without the feature
does not provide equivalent evidence through an inferred tool list.

## Project planning response

The MCP operator tools `projects.session_capabilities` and
`sessions.capabilities` call these same read-only routes and accept their
query arguments. They neither assemble turns nor bypass the local-owner checks.

`GET /api/projects/:id/session-capabilities?host=local&skill_preset=rupu-session-v1`

`host` may be omitted or `local`. `skill_preset` may be omitted (the default
`rupu-session-v1`), `rupu-session-v1`, or `none`. Unknown query fields and preset
values are rejected. Remote hosts require a direct connection to their owning
control plane.

The route resolves the registered project's canonical root, then reads the
global and that project's `.rupu/skills` layers. It does not discover an
ancestor project or resolve provider configuration. The bundled harness's
explicit tool declaration is returned separately from skill selection.

This is **illustrative JSON**, not a captured runtime or UI acceptance result:

```json
{
  "schema_version": 1,
  "host_id": "local",
  "workspace_id": "ws_example",
  "preset": { "id": "rupu-session-v1", "version": 1, "enabled": true },
  "skills": {
    "preset": { "id": "rupu-session-v1", "version": 1, "enabled": true },
    "selected": [
      {
        "name": "coverage-assets",
        "layer": "global",
        "preload": false,
        "indexed": true,
        "reason": "session",
        "stamp": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
      },
      {
        "name": "rupu-session-operations",
        "layer": "global",
        "preload": true,
        "indexed": false,
        "reason": "session",
        "stamp": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        "content_hash": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
      }
    ],
    "defaults": ["global:coverage-assets", "global:rupu-session-operations"],
    "preload_defaults": ["global:rupu-session-operations"],
    "diagnostics": [
      {
        "entry": "global:finding-report",
        "layer": "global",
        "code": "not_installed",
        "message": "optional preset skill global:finding-report is not installed"
      }
    ]
  },
  "declared_tools": ["read_file"],
  "permission_mode": "readonly",
  "applies": "new_session",
  "effective": null,
  "limitations": ["Declared tools are not evidence of an assembled grant, constructed services, credentials, or target access."]
}
```

The illustrative tool list, selected skills, and diagnostics are abbreviated.
The actual tool list comes from `rupu_agent::builtin::session_agent().tools`.
The exact preset members are `global:coverage-assets`, `global:finding-report`,
`global:rupu-session-operations`, and `global:work-units`. Only installed,
valid, model-loadable members are selected. The operations skill is preloaded
by default so its operating instructions are present from the first turn.
The other three skill bodies stay indexed and load on demand, keeping their
larger instructions out of the initial prompt. Skills never add tool grants.
The response uses `effective: null` because no turn was assembled.

`skills.defaults` contains every added preset grant, including the operations
skill. The additive `skills.preload_defaults` field lists the subset to
preload: only `global:rupu-session-operations` when eligible and not already
explicitly selected. Clients must accept an absent `preload_defaults` as an
empty list for compatibility with earlier v1 responses. The frozen
`selected` entries remain authoritative for each skill's actual `preload`
flag and optional `content_hash`. Preloaded skills have `indexed: false` because
their instructions are already present in the prompt. With `skill_preset=none`, both default lists
are empty.

## Existing session response

`GET /api/sessions/:id/capabilities?host=local`

Only the optional local `host` query is accepted. Active and archived sessions
are supported. A duplicate identity present in both stores is a conflict. The
record ID and supported version must match; settings, orchestration, and
capability evidence are read from that exact session's directory.

This is **illustrative JSON** for an idle session with no assembled turn:

```json
{
  "schema_version": 1,
  "session_id": "ses_example",
  "host_id": "local",
  "workspace_id": "ws_example",
  "archived": false,
  "preset": null,
  "preset_diagnostics": [],
  "declared_tools": [],
  "settings_revision": 0,
  "orchestration_revision": 0,
  "permission_mode": "readonly",
  "skills": { "ready": true, "entries": [], "diagnostics": [] },
  "effective": null,
  "effective_settings_revision": null,
  "effective_orchestration_revision": null,
  "effective_current": false,
  "effective_notice": "No assembled-turn capability snapshot has been recorded for this session; effective tools and services are unobserved."
}
```

`preset` is the nullable marker saved at creation. `preset_diagnostics` retains
creation-time omissions such as optional preset members that were not
installed. Installing those skills later neither clears these historical
diagnostics nor expands the frozen selection. Legacy records are not
assigned a preset during inspection. `declared_tools` is the session settings
override when present, including an explicit empty array; otherwise it is the
frozen agent declaration, which may itself be null. A declaration does not
include every ambient grant and is not a model tool availability claim.

`skills` is `FrozenSkillReadiness`: current inspection of the stored frozen
selection, without expanding a wildcard, rerunning the preset, or adding
newly installed members. Its fields are:

| Field | Type | Meaning |
| --- | --- | --- |
| `ready` | boolean | Every frozen entry can currently be resolved; a content change alone does not block it. Missing optional preset members outside the frozen set do not affect this field. |
| `entries` | array of `FrozenSkillStatus` | One row per stored frozen entry. |
| `diagnostics` | array of `SkillDiagnostic` | Discovery or resolution diagnostics. |

`FrozenSkillStatus` contains `entry` (qualified name), `frozen` (`FrozenSkill`),
`installed` (nullable boolean), `valid` (nullable boolean), `model_loadable`
(boolean), `current_stamp` (nullable string), `changed` (nullable boolean), and
`missing` (nullable diagnostic code). Null installed/valid/change values mean
the information was not established; clients must not convert them to false
or infer availability. A frozen record contains `name`, `layer`, `preload`,
`indexed`, `reason`, `stamp`, and optional `content_hash` for a preload.

`SkillDiagnostic` contains nullable `entry`, `layer`, `code`, and `message`.
Codes are `not_installed`, `invalid`, `not_model_loadable`, `unreadable_layer`,
`removed`, `changed`, `invalid_frozen_record`, and `conflict`.

## Observed turn evidence

`effective` is a nullable `RunReadiness` snapshot written after successful
assembly of one session turn. It reports the actual grant and constructed
services from that assembly. The API never assembles a turn to populate it.

This is **illustrative JSON** for the `effective` object; inventories are
abbreviated and this is not evidence that an operation was exercised:

```json
{
  "schema_version": 1,
  "run_id": "run_example",
  "agent": "rupu-session",
  "provider": "example-provider",
  "model": "example-model",
  "observed_at": "2026-11-06T00:00:00Z",
  "applies": "assembled_turn",
  "permission_mode": "readonly",
  "approval_channel_available": false,
  "tools": [
    {
      "name": "read_file",
      "effect": "read",
      "selected": true,
      "offered": true,
      "grant_reasons": ["declared"],
      "narrowed": false,
      "unavailable": false,
      "skipped": false,
      "required_services": [],
      "missing_services": [],
      "service_available": true,
      "permission": "allowed",
      "permission_degraded": false,
      "child_ceiling": null
    }
  ],
  "skills": [
    {
      "qualified_name": "global:coverage-assets",
      "reason": "session",
      "indexed": true,
      "preload": false,
      "model_loadable": true,
      "baseline_stamp": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
    }
  ],
  "connectors": [
    {
      "platform": "github",
      "capability": "repository",
      "registry_service_constructed": true,
      "known_accounts": ["example-account"],
      "target_access": "unverified"
    }
  ],
  "limitations": ["Constructed connector accounts do not verify authentication, target access, or operation permissions."]
}
```

Tool rows distinguish selection, offered grant membership, narrowing,
unavailability, skipping, service construction, and effect-level permission.
`selected` includes ambient grants and does not mean explicitly declared.
`offered` is exact membership in the grant offered to the model. Grant reasons
are retained for offered entries only. `permission` is `allowed`,
`denied_readonly`, `approval_required`, `approval_unavailable`, or
`spawn_with_ceiling`. It is not authorization for a particular target.
`child_ceiling` is the spawn policy ceiling when applicable. A constructed
connector account always has `target_access: "unverified"`; the inventory
covers repository handles, not every vendor feature.

The worker stores `capabilities.json` beside the owning `session.json` with
the exact envelope `{schema_version: 1, session_id, workspace_id,
settings_revision, orchestration_revision, effective}`. All envelope fields
are required and unknown fields are rejected. The API verifies both schema
versions, `applies: "assembled_turn"`, the session/project identity, and that
the effective run belongs to the record's `runs` list. Corrupt, unsupported,
partial, and foreign evidence returns a conflict instead of availability.

`effective_current` is true only when the snapshot's run equals the current
`active_run_id`, or `last_run_id` when no active run exists, and both snapshot
revisions equal the session's current settings and orchestration revisions.
Historical snapshots remain visible with `effective_current: false` and an
explicit notice. Even a current snapshot describes an assembled turn; it
does not predict the next turn. Missing evidence returns null and an
unobserved notice. No credentials, messages, or system prompts are returned.

## Creation inputs and client behavior

The project-owned session creation body accepts optional `skill_preset`,
`skills`, and `preload_skills` fields alongside its existing launch inputs.
`skill_preset` accepts `rupu-session-v1` (the default) or `none`. API skill
entries must be exact qualified `global:name` or `project:name` selections;
wildcards are rejected. `skills` selects indexed skills and `preload_skills`
explicitly requests preload. An explicit same-name selection preserves its
own layer and preload flags; the preset does not upgrade an explicit indexed
operations skill to preload. Opting out removes preset additions, including
the default operations preload, while preserving explicit selections. These
selections are frozen at creation. The
capability planning GET has no explicit-selection query fields; its preview
describes only the chosen default preset for the bundled harness.

Clients should show declared tools, current frozen skill inspection, and
observed turn evidence as separate facts. Preserve null, false, and empty
arrays distinctly. Display missing skill diagnostics and staleness notices
without converting them into provider or connector health claims. Do not use
this feature flag as a creation-adapter capability: existing launch feature
gates still decide whether a deployment can create or run a session.

### Creation adapter gate

`session.skill_preset.v1` is advertised only when the attached session starter
supports the preset and explicit skill fields. It is separate from the read
feature `session.capabilities.v1`. Read-only deployments still support capability
inspection. To send `skill_preset`, `skills`, or `preload_skills` when creating a
project session, clients require `session.skill_preset.v1` as well as the existing
creation feature (`session.builtin_harness`, and `session.idle_harness` for idle).
Older starters reject these new fields before launching a child process.
