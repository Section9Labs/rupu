# Full Rupu instructional preset

`rupu-session-full-v1` is an explicit new-session skill preset. It names the
109 skills in Section9Labs/rupu-skills at
`d57aa11a8964202ab28d1c21818c97dcedc0d6f1`, plus Rupu's locally authored
`rupu-session-operations`. The checked-in inventory records each upstream
SKILL.md hash and its declared version/compatibility when present. This is a
security-methodology suite, not an inventory of every Rupu runtime feature.

The preset adds only installed, valid, model-loadable exact `global:name`
entries. It indexes them on demand; operations remains preloaded. Missing,
invalid and explicit-use-only entries produce the existing diagnostics, without
installing anything. Project skills never silently shadow global entries.
An explicit qualified selection overrides a preset member of the same name;
ambiguous explicit names still fail under the existing resolver.

Omitted preset, `rupu-session-v1`, and `none` retain their existing behavior.
Existing session skill snapshots never expand when more skills are installed.
The full preset is frozen before the first turn, and changes no tool declaration,
MCP selection, permissions, credential access or external service setup.
Membership is classification by the pinned exact-name inventory, not an
attestation that installed contents still equal upstream hashes.

## Wire contract

The existing project session-capabilities GET and `projects.session_capabilities`
operator MCP tool accept `skill_preset=rupu-session-full-v1`. The response retains
`skills.selected`, `defaults`, `preload_defaults` and `diagnostics`, and adds
`skills.inventory` only for the full preset:

- `upstream_repository`: `https://github.com/Section9Labs/rupu-skills`
- `upstream_commit`: the exact pinned commit above
- `expected_entries`: 110 exact `global:name` strings
- `local_entries`: `global:rupu-session-operations`, which is not from that upstream repository

Plan reads remain read-only and do not imply that creation is available.
Creating a full-preset session requires affirmative
`session.skill_full_preset.v1`; legacy `session.skill_preset.v1` is insufficient.
The new feature requires a writable builtin-harness adapter that explicitly
supports full presets. POST sends the existing `skill_preset` field with the new
value; the CLI accepts the corresponding `--skill-preset` value. Full choices
are refused before dispatch when the adapter does not support them.

A client can show available/expected counts and a compact missing list, linking
to its existing Manage Skills flow. It must not present inventory inclusion as
loaded instructions, granted tools, working binaries, or usable services. Native
clients may choose full as their new-session default only on affirmative support;
this does not change the backend omission contract or overwrite saved choices.

## Installation review

Use the existing `/api/skills/install` operation, source repository and exact
commit, `host=local`, global layer, `force=false`. First request `dry_run=true`;
then review exact missing names and submit those names explicitly. Do not replace
existing entries implicitly, import unrelated repositories, install executables,
or change permissions. Operations is managed separately from the upstream suite.
After an ambiguous write, read the catalog/source records to reconcile rather
than repeating the mutation. Installing instructions does not execute them.

## Validation status

Source-only implementation and tests prepared; no build or test execution is
claimed in this worktree. Focused validation should cover runtime full/legacy
presets, CLI session skill freeze and adapter argv, project creation feature
checks, capability reads including operator MCP parity, and the route inventory.
