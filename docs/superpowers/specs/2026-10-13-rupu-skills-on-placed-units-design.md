# Skills on placed units: ship skill content with the dispatch

**Status:** draft for review, 2026-10-13.
**Builds on:** the Agent Skills spec (`docs/superpowers/specs/2026-10-12-rupu-agent-skills-design.md`), whose §9 left shipping skill content as "a later option if this proves painful". matt decided on 2026-10-13 that placed units carry their skills.

## Problem

A placed unit (a step with `host:` or `distribute:`) is dispatched by name (`crates/rupu-orchestrator/src/runner.rs:124`). The host runs `rupu run <agent>` and resolves skills from its own layers: its global `~/.rupu/skills` and the staged workspace's `.rupu/skills`. Today that means:

- Every global skill a unit uses must be installed on every host it can land on. If one is missing, the unit is recorded unstarted.
- The host's copy may be a different version from the coordinator's, and nothing notices.
- Tunnel and bucket hosts have no workspace sync (`crates/rupu-cp/src/host/connector.rs:626`), so their units can never get a project skill.
- Tar-mode workspace sync drops symlinks (`crates/rupu-workspace/src/workspace_sync.rs:128`), and a gitignored `.rupu/` sends no project skills at all.

The case that makes this matter is recon. The recon agent is the one most often placed on a host near the target network, and its methodology now lives in skills (the planned `Section9Labs/rupu-skills` repo). A recon unit should run on a fresh host without anyone installing skills there.

## Goal

The coordinator sends the content of the skills a placed unit needs, and the host runs the unit with exactly those copies. A host that already holds a copy of the same content receives nothing again.

## Non-goals

- **Shipping agent files.** The host still loads the agent by name, as today. This spec ships skills only.
- **Making the host's installed skills unusable.** A skill the coordinator did not ship still resolves from the host's own layers, as today.
- **Cache eviction.** The host cache is content-addressed and holds only distinct skill versions. A prune command can follow if it grows.
- **Sessions on remote hosts.** A remote session resolves skills on its own host (Plan 3 and 4 behaviour). This spec covers placed workflow units.
- **`host: local`.** A unit launched on the coordinator's own machine already sees the coordinator's layers. Nothing is shipped.

## Decisions (for matt's review)

| # | Decision |
|---|---|
| P1 | The coordinator resolves what to ship: the skills named by its own copy of the unit's agent (`skills:` / `preloadSkills:`) plus the step's `skills:` / `preload_skills:`, with the same grammar and S9 rules as a local run (`*` = the coordinator's global layer). When the coordinator has no copy of the agent, it ships the step's skills only. |
| P2 | Shipped skills **overlay** the host's layers. The host still resolves the agent's grants itself, against its global layer, its staged project layer, and the overlay. A shipped `global:x` replaces the host's own `global:x`, so the coordinator's version wins. A grant the overlay doesn't cover falls back to what the host has installed, and fails closed as today if the host has nothing. |
| P3 | Skills are shipped as one tar per skill, named by its `content_hash` (the existing `rupu_skills::tree::content_hash`). The host verifies the hash after unpacking and keeps the copy in a content-addressed cache. |
| P4 | Before sending, the coordinator asks the host which hashes it lacks and sends only those. Within one coordinator process, a host that has confirmed a hash is not asked about it again, and concurrent units bound for the same host share one transfer. |
| P5 | A peer advertises the new capability `agent.skill_bundle`. If a unit has skills to ship and its peer lacks the capability, the unit is dispatched by name as today, and the run gets an `Event::StepWarning`. The warning names the host and says the unit will use the host's installed copies. That is a visible downgrade, not a silent drop. If the host then lacks a skill, the unit fails closed as today. |
| P6 | Caps: a shipped skill's tree is at most 32 MiB, and a unit's whole bundle is at most 128 MiB. The coordinator refuses anything over either cap before dispatch, and the message names the skill and the cap. |

P2 versus "the shipped set replaces resolution": replacing would mean the host ignores its own agent file's `skills:`. Overlay keeps the rule that the agent the host loads decides what it is granted, while the coordinator decides which version of each skill it gets. P5 versus refusing: refusing breaks setups that work today, such as a host with skills installed and an older rupu. Flip either decision before the plan if you prefer the other.

## 1. What the coordinator ships

`UnitDispatch` already carries the step's `skills` / `preload_skills` (`crates/rupu-orchestrator/src/runner.rs:157-162`). At dispatch, before the connector is called, the fleet unit dispatcher (`crates/rupu-cli/src/fleet_unit_dispatcher.rs:316`) does the following:

1. Loads the coordinator's copy of the unit's agent, if it has one, and resolves `SkillRequest`s for the agent's keys and the step's entries with `rupu_skills::run::resolve_run_skills` over the coordinator's `LayerDirs`. This is the same call a local run makes, with reason `launch`, on the blocking pool with the fd retry.
2. If resolution fails, for example a step skill that isn't installed on the coordinator, the unit is recorded unstarted with the coordinator-side error. This is where the local run would have failed too.
3. For each granted skill, it builds a `ShippedSkill { layer, name, content_hash, stamp, bytes }`, where `bytes` is the length of the packed tar. It packs at most once per content hash per process.

The result is a `SkillBundle { skills: Vec<ShippedSkillRef> }` manifest (layer, name, content hash) plus the tars. It rides on `AgentLaunchRequest.skill_bundle` (`crates/rupu-cp/src/agent_launcher.rs`) and on `RunSpec.skill_bundle` for tunnel and bucket.

`skills: ["*"]` ships every skill in the coordinator's global layer. The P6 cap bounds it, and the unit's `skill_index` records what was granted.

## 2. Pack format

A skill packs to a ustar archive holding exactly what `rupu_skills::tree::walk_tree` (`crates/rupu-skills/src/tree.rs:85`) records:

- Each entry is either a regular file or a symlink.
- Each path is relative to the skill root.
- File modes are normalised to 0755 or 0644, as the walk already does.
- Symlink targets are relative and stay inside the root, because the walk already refuses any other target.
- `.git` and `.DS_Store` are skipped, as in the walk.

Because the archive holds what the content hash covers, unpacking it and re-walking reproduces the same `content_hash`. That is the verification in §3.

`pack_skill(&Skill) -> Vec<u8>` and `unpack_skill(bytes, dest) -> Result<ContentHash, UnpackError>` live in `rupu-skills` (new module `ship.rs`). They are pure and are the only code that touches the format.

## 3. The host cache and overlay

The host keeps shipped skills in `<RUPU_HOME>/cache/skills/<content_hash>/<name>/`, in a directory created 0700.

`unpack_skill` writes into a temporary directory next to the cache and refuses:

- an entry path that is absolute or contains `..`;
- a symlink whose target leaves the skill root;
- any entry type other than file, directory or symlink;
- a tree over the P6 cap, counted while unpacking, not after.

Then it walks the result, computes `content_hash`, and renames the directory into place only when the hash matches the name it was shipped under. A mismatch is an error, and nothing is kept. If two units unpack the same hash at once, the second rename fails because the first copy is already there, and the second unit uses that copy.

At launch, the host gets the manifest as a hidden `rupu run --skill-bundle <file>` flag, which `RunArgv::Agent` renders and `argv_roundtrip` covers. The file is a JSON copy of the manifest that the connector writes into the host's staging area, and the run copies it into its own run directory so that `rupu run --continue` on the host finds the same overlay. Every entry must be present in the cache. A missing one fails the launch with a message naming the host and the skill.

Resolution on the host gains the overlay as a third input:

- `rupu_skills::LayerDirs` gains `overlay: Vec<OverlaySkill { layer, name, dir }>`.
- `SkillCatalog::discover` lists overlay entries in their layer and drops any installed entry with the same layer and name.
- The S9 rules apply to the merged listing unchanged: a name bound in both layers must be qualified, and `*` covers the global layer only.
- `load_agent`'s name check (`crates/rupu-agent/src/loader.rs:149`) reads the same merged listing, so an agent whose skills only arrive by shipping still loads.
- The assembler (`RunAssembler::resolve_skills`, `crates/rupu-runtime/src/assembly/mod.rs:514`) builds `LayerDirs` with the overlay when the spec carries one.

Read roots, `skills.load` and compaction re-attach need no change. They work from a granted skill's directory, which is now a cache directory. That directory never changes, so `skills.load` never reports `changed` for a shipped skill.

## 4. Transports

Each connector gets one new required method on `HostConnector`:

```rust
async fn ship_skills(&self, bundle: &PackedBundle) -> Result<(), HostConnectorError>;
```

It runs before `launch_agent`, and only when the unit has skills to ship and the peer advertised `agent.skill_bundle`. Each implementation asks which hashes the host lacks, sends those, and returns when the host has confirmed every hash in its cache. A shared `ShippedMemo` keyed by host id records the confirmed hashes and holds a single-flight per (host, hash), so a fan-out of many units to one host makes one transfer per skill (P4).

- **SSH.** One command does both steps: `rupu __skills receive`. It reads a header listing the hashes on stdin, writes the missing ones to stdout, then reads those tars and replies `ok <hash>` per stored skill. That is one ssh connection per batch, not one per skill, so a fan-out never bursts connections at a host. An SSH remote advertises `agent.skill_bundle` through `rupu __features`. Since the SSH connector has no feature report before running a command, `rupu __skills receive` itself is the gate. An older remote fails with its own clap error, which the connector maps to the P5 downgrade.
- **HTTP.** `POST /api/skills/bundle/missing` takes `{hashes}` and returns the missing ones. `PUT /api/skills/bundle/:hash` takes the tar as the body, verifies it and stores it. Both are behind the peer's token, and the capability comes from `/api/host/info` `features`.
- **Tunnel.** New frames: `SkillsOffer { id, hashes }`, then `SkillsNeed { id, hashes }`, then `SkillChunk { id, hash, seq, data, last }`, then `SkillsStored { id, hashes }`. Chunks are base64 of at most `ARTIFACT_CHUNK_BYTES` (1 MiB), mirroring `ArtifactPull` / `ArtifactChunk` (`crates/rupu-cp/src/node/protocol.rs:61-77`) in the other direction. The capability is advertised in `Hello.capabilities`. As built, the frames carry full refs (layer, name, hash) instead of bare hashes, because the host cache key is `<hash>/<name>` and the hash does not cover the name. Bucket objects stay keyed by hash. See Plan 2, "Deviation from the spec".
- **Bucket.** The coordinator puts `skills/<hash>.tar`, skipping an object that already exists. The job's `RunSpec.skill_bundle` lists the manifest. The worker downloads any hash it lacks before starting the run and verifies each as in §3. The capability is advertised in the worker marker.
- **Local.** `host: local` ships nothing. Its launcher passes no `--skill-bundle`.

A transfer failure fails the unit's launch, recorded as a launch error with the connector's message. It never falls back to host-installed copies, because the peer said it could take them.

## 5. Transcript and views

`SkillIndexEntry` (`crates/rupu-transcript/src/event.rs:71`) gains `shipped: bool`, with a serde default of false and skipped when false. The CLI and web renderers show `(shipped)` after such a skill. A unit's coordinator-side events gain `Event::StepWarning` for the P5 downgrade.

The §10 gap is fixed on the way. The Agent Skills spec says a missing skill on a remote host gives a message that names the host, but today's message names only directories. Every coordinator-side failure for a placed unit's skills (resolve, pack, cap, transfer, missing on host) is prefixed with the host id.

## 6. Errors

| Condition | Result |
|---|---|
| A step skill isn't installed on the coordinator | The unit is recorded unstarted on the coordinator, as a local run would be. |
| A skill is over the per-skill cap, or the bundle over the total cap | The unit is recorded unstarted, naming the skill and the cap. |
| The peer lacks `agent.skill_bundle` | Dispatch by name, plus a `StepWarning` naming the host (P5). |
| A transfer fails or a hash mismatches on the host | Launch error with the host id and the reason. Nothing is cached. |
| A manifest entry is missing from the host cache at launch | Launch error naming the host and the skill. |
| The host's agent grants a skill nobody shipped and the host lacks | The unit is recorded unstarted on the host, as today, with the message now prefixed by the host id. |

## 7. Tests

- **Pack round trip (`rupu-skills`).** Pack and then unpack reproduces `content_hash`, including relative symlinks. Unpacking refuses `..`, absolute paths, an escaping symlink, a device entry and an over-cap tree, and leaves nothing behind. A tampered tar fails the hash check.
- **Overlay resolution.** A shipped `global:x` replaces the installed `global:x`. A bare name bound by an overlay project skill and an installed global skill is refused as ambiguous. `*` includes overlay global skills and no overlay project skill. `load_agent` accepts an agent whose skill exists only in the overlay.
- **Coordinator resolution.** An agent's keys plus step entries resolve on the coordinator. A missing step skill records the unit unstarted with the coordinator's message. With no local copy of the agent, only step skills ship.
- **Dedup.** Twenty concurrent units for one host make one transfer per hash (counted on a scripted connector). A second dispatch in the same process asks nothing.
- **Per transport.** SSH: `__skills receive` against a scripted remote, plus the old-remote downgrade. HTTP: an httpmock peer with and without the feature. Tunnel and bucket: the existing in-process harnesses, including a worker that lacks a hash. `argv_roundtrip` covers `--skill-bundle`.
- **End to end (`rupu-cli` `tests/serial/`).** A placed unit on a scripted SSH host whose global layer lacks the skill runs, loads the shipped skill, and records `shipped: true` in its `skill_index`.

## 8. Plans

1. **Format, cache, overlay, SSH and HTTP.** `ship.rs` (pack and unpack), the host cache, the `LayerDirs` overlay through discovery, `load_agent` and the assembler, `--skill-bundle`, coordinator-side resolution and caps, `ShippedMemo`, the `ship_skills` method with SSH and HTTP implementations, the capability and the P5 downgrade, transcript `shipped`, and the host-id prefix.
2. **Tunnel and bucket.** The new frames, the bucket `skills/` objects and worker download, and their capabilities and tests.
