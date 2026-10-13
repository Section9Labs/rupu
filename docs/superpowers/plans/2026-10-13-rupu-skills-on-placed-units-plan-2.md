# Skills on placed units, Plan 2: tunnel and bucket

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A placed unit on a tunnel (`rupu node`) or bucket (dead-drop worker) host runs with the coordinator's copies of its skills, as SSH and HTTP hosts already do after Plan 1.

**Architecture:**
- **Tunnel.** New frames carry an offer of skill refs, the node's answer of what it lacks, the chunks, and the node's confirmation. The coordinator correlates them with a request id, the way `ArtifactPull` is correlated today. The node assembles chunks in memory under the Plan 1 caps and stores each skill with `rupu_skills::ship::store` on the blocking pool.
- **Bucket.** The coordinator puts `skills/<hash>.tar` objects (create-if-absent). The job's `RunSpec` carries the manifest. The worker downloads any skill its cache lacks before it starts the run.
- **Both.** `RunSpec` gains `skill_bundle`, which `run_argv` renders as `rupu run --skill-bundle`. Nodes and bucket workers advertise `agent.skill_bundle`. Plan 1's `not_built` / `refuse_bundle_launch` stubs go away.

**Tech Stack:** Rust (tokio, axum 0.7 WebSocket, tokio-tungstenite 0.24, object_store, serde, base64).

**Spec:** `docs/superpowers/specs/2026-10-13-rupu-skills-on-placed-units-design.md` (§4, decisions P1–P6). Plan 1 (`docs/superpowers/plans/2026-10-13-rupu-skills-on-placed-units-plan-1.md`, complete) built everything this plan reuses.

## Global Constraints

- Capability: `agent.skill_bundle` (`CAP_AGENT_SKILL_BUNDLE`, `crates/rupu-cp/src/node/protocol.rs`). Plan 2 adds it to `node_capabilities()` (tunnel `Hello`) and `bucket_worker_capabilities()` (worker marker). The test `only_host_features_advertise_the_skill_bundle` flips to "every executor advertises it".
- Caps (P6): `MAX_SHIPPED_SKILL_BYTES` (32 MiB) per skill tree. `MAX_BUNDLE_BYTES` (128 MiB) per unit. A receiver also caps the archive bytes it accepts per skill at `MAX_SHIPPED_SKILL_BYTES + 64 KiB`, as the HTTP PUT does.
- Every skill a host receives goes through `rupu_skills::ship::store`, which verifies the content hash before anything counts as cached. `store` is blocking, so call it from `spawn_blocking`.
- Never send a new frame to a node that didn't advertise `agent.skill_bundle`. An older node's frame parse is fatal to its connection (`crates/rupu-cli/src/cmd/node.rs` `parse_frame(&msg)?`).
- `RunSpec` has no `deny_unknown_fields`, so an older executor would silently drop `skill_bundle`. The sender-side capability gate is therefore the only guard, and `launch_agent` must refuse a bundle to a peer that lacks the capability.
- P5, tunnel and bucket: a peer without the capability is a visible downgrade (`ShipOutcome::Unsupported`), never a failure. The connector checks the capability BEFORE `ship_through`, so the downgrade is NOT memoised: a node that upgrades and reconnects, or a worker whose marker changes, is used on the next unit. A transfer failure on a peer that advertised the capability is an error.
- P4: `ShipMemo` on each tunnel and bucket connector instance. A hash a host confirmed is not offered again in the same process.
- Integration tests are modules under `crates/<c>/tests/it/`. rupu-cli env- and cwd-mutating tests go in `tests/serial/`. Never run bare `git stash`. Format only the files you touch. `cargo clippy -p <crate> --all-targets` must be clean.

## Deviation from the spec (decided here)

The spec's tunnel frames carry hashes only (`SkillsOffer { id, hashes }`). The host cache key is `<hash>/<name>`, and the content hash doesn't cover the name, so two skills with the same content under different names need separate entries. The frames therefore carry full `ShippedSkillRef`s (layer, name, hash), as HTTP's `missing` body and SSH's `hash:layer:name` arguments already do. Bucket objects stay keyed by hash (`skills/<hash>.tar`), because the tar doesn't depend on the name and the worker takes names from the job's manifest.

## Review Focus

1. **A node disconnects in the middle of an offer.** The coordinator's wait ends with `Unreachable` (not a hang), nothing is memoised as confirmed, and the next unit offers again. The node drops the offer's buffers with the connection (Task 3).
2. **A hostile or buggy coordinator sends a node chunks that are out of order, for a ref it never asked for, or past the cap.** The node answers that offer with an error and stores nothing, and the connection stays up (Task 2).
3. **A bucket worker sees a job whose `skills/<hash>.tar` object is missing or fails verification.** The job is marked failed with a message naming `layer:name`. A transient bucket read error leaves the job pending for the next tick (Task 5).
4. **Two units bound for one node at once.** One offer goes out; the second waits on the connector's ship lock and finds its hashes confirmed (Task 3).
5. **An old node or worker (no capability).** Units downgrade with the warning and launch by name. A direct `launch_agent` carrying a bundle is refused before any frame or job is written (Tasks 3 and 4).

---

### Task 1: Protocol: frames, `RunSpec.skill_bundle`, capabilities

**Files:**
- Modify: `crates/rupu-cp/src/node/protocol.rs`
- Test: the protocol.rs unit tests

**Interfaces:**
- Produces:
  ```rust
  // Frame (serde tag = "type", snake_case), new variants:
  SkillsOffer { id: String, skills: Vec<rupu_skills::ship::ShippedSkillRef> },        // CP -> node
  SkillsNeed { id: String, skills: Vec<ShippedSkillRef>, #[serde(default, skip_serializing_if = "Option::is_none")] error: Option<String> }, // node -> CP
  SkillChunk { id: String, layer: Layer, name: String, hash: String, seq: u64, data_b64: String, last: bool }, // CP -> node
  SkillsStored { id: String, #[serde(default)] stored: Vec<ShippedSkillRef>, #[serde(default, skip_serializing_if = "Option::is_none")] error: Option<String> }, // node -> CP
  // RunSpec
  #[serde(default, skip_serializing_if = "Option::is_none")] pub skill_bundle: Option<rupu_skills::ship::SkillBundleManifest>,
  ```
- `RunSpec::run_argv`: in the agent branch, set `run.skill_bundle = Some(serde_json::to_string(m)?)` when `skill_bundle` is `Some` (the same JSON `AgentLaunchRequest::agent_run()` renders). The workflow branch refuses a bundle with the same wording it uses for `skills`.
- `node_capabilities()` and `bucket_worker_capabilities()` include `CAP_AGENT_SKILL_BUNDLE`. Update the capability doc comment to say where it's advertised.

- [ ] **Step 1: Write failing tests:**
  - every new frame round-trips through JSON, with `type` tags `skills_offer`, `skills_need`, `skill_chunk` and `skills_stored`;
  - a `RunSpec` with no bundle serializes byte-identically to today (no `skill_bundle` key);
  - an agent `RunSpec` with a bundle renders exactly one `--skill-bundle=` element whose JSON parses back to the manifest;
  - a workflow `RunSpec` with a bundle is refused;
  - flip `only_host_features_advertise_the_skill_bundle` to assert all three lists include it.
- [ ] **Step 2: Run `cargo test -p rupu-cp --lib protocol` and confirm the failures.**
- [ ] **Step 3: Implement.** Grep every `RunSpec {` literal across `crates/` and add `skill_bundle: None`.
- [ ] **Step 4: Run `cargo test -p rupu-cp --lib`, `cargo test -p rupu-cli --lib node` and clippy.**
- [ ] **Step 5: Commit** `feat(node): skill-shipping frames and RunSpec.skill_bundle`.

Nothing sends the new frames yet, but the advertised capability is real only once Task 2 lands, so Tasks 1 and 2 ship in one PR.

---

### Task 2: The node receives skills

**Files:**
- Modify: `crates/rupu-cli/src/cmd/node.rs` (`connect_and_run`'s frame loop, ~:551–:785)
- Test: the node.rs tests (fake CP: `accept_node`, `spawn_node`, `next_cp_frame`, `cp_text`, ~:4366–:4421)

**Interfaces:**
- Consumes: the Task 1 frames, `rupu_skills::ship::{cache_dir, missing, store, ShippedSkillRef, MAX_SHIPPED_SKILL_BYTES, MAX_BUNDLE_BYTES}`.
- Produces the node half of the exchange:
  1. On `SkillsOffer { id, skills }`: validate every ref (`ShippedSkillRef::validate`). An invalid ref, a duplicate (layer, name), or a second offer reusing a live `id` answers `SkillsNeed { id, skills: [], error: Some(..) }`. Otherwise answer `SkillsNeed { id, skills: missing(cache_dir(global), &skills) }`, and remember the needed set under `id`. An offer needing nothing is finished at once (no `SkillsStored` follows).
  2. On `SkillChunk`: the chunk must name a ref in that offer's needed set, with `seq` equal to the next expected for that ref. Base64-decode it and append to that ref's buffer. Refuse a ref's buffer past `MAX_SHIPPED_SKILL_BYTES + 64 KiB`, or an offer's total past `MAX_BUNDLE_BYTES + needed.len() * 64 KiB`. On `last`, hand the bytes to a `tokio::task::JoinSet` that runs `spawn_blocking(store(..))`.
  3. When every needed ref has stored, send `SkillsStored { id, stored: needed }`. The first failure (a bad chunk, a cap, an unknown id, a store/verify error) sends `SkillsStored { id, stored: [], error: Some(msg naming layer:name) }` and drops the offer's state.
  4. The connection never closes over a skill error.
  5. The `JoinSet` is polled in the loop's `tokio::select!`, so a store never stalls inbound frames.
  6. Offer state lives in the connection loop and is dropped with it.

- [ ] **Step 1: Write failing tests**, driving a real `connect_and_run` against a fake CP with a temp `global`:
  1. an offer of two refs where one is pre-stored answers `SkillsNeed` with only the other; that skill's chunks (split across two `SkillChunk`s) produce `SkillsStored` and `global/cache/skills/<hash>/<name>/SKILL.md`;
  2. chunks for an unasked ref, an out-of-order `seq`, a tampered archive, and an over-cap ref each give `SkillsStored { error: Some }`, store nothing, and the connection still answers a following `Ping`;
  3. a `Run` frame with a `skill_bundle` spawns `rupu run … --skill-bundle=…`. Use the existing test that checks the spawned argv, or the `exe` stub pattern.
- [ ] **Step 2: Run `cargo test -p rupu-cli --lib node` and confirm the failures.**
- [ ] **Step 3: Implement.** Add the new CP→node variants to the frame match. Add the node→CP-only variants (`SkillsNeed`, `SkillsStored`) to the "unexpected from CP" arm (~:773).
- [ ] **Step 4: Run `cargo test -p rupu-cli --lib node` and clippy on rupu-cli.**
- [ ] **Step 5: Commit** `feat(node): rupu node receives shipped skills`.

---

### Task 3: The coordinator ships over the tunnel

**Files:**
- Modify: `crates/rupu-cp/src/node/registry.rs` (`NodeConn`: an offer table beside `pulls`), `crates/rupu-cp/src/node/server.rs` (route `SkillsNeed` / `SkillsStored`), `crates/rupu-cp/src/host/tunnel.rs` (`TunnelHostConnector.ship_memo`, `ship_skills`, `launch_agent`)
- Modify: `crates/rupu-cp/src/host/skill_ship.rs` (delete `not_built` / `refuse_bundle_launch` once bucket no longer uses them, in Task 4)
- Test: `crates/rupu-cp/tests/it/node_tunnel.rs` (`connect_pull_capable_node`, `setup_with_capabilities`, `route_*` helpers), plus the tunnel.rs unit tests (replace `shipped_skills_are_unsupported_and_a_bundle_launch_is_refused`)

**Interfaces:**
- Consumes: Task 1 frames; `skill_ship::{ship_through, Transfer, ShipMemo, SkillBundle}`.
- Produces:
  ```rust
  // NodeConn
  pub enum OfferMsg { Need { skills: Vec<ShippedSkillRef>, error: Option<String> }, Stored { stored: Vec<ShippedSkillRef>, error: Option<String> } }
  pub fn begin_offer(&self, id: &str) -> mpsc::Receiver<OfferMsg>;   // capacity 4
  pub fn end_offer(&self, id: &str);
  pub async fn route_offer(&self, id: &str, msg: OfferMsg);
  // close_pulls() also closes every open offer (the receiver sees None)
  ```
- **`TunnelHostConnector::ship_skills`:**
  1. Empty bundle → `NotNeeded`.
  2. No live connection → `Err(Unreachable)`.
  3. `!conn.supports(CAP_AGENT_SKILL_BUNDLE)` → `Ok(Unsupported("host {id}: node {node_id} (rupu {ver}) can't receive shipped skills; upgrade rupu there"))`, checked BEFORE `ship_through` so it is not memoised.
  4. Otherwise run `ship_through(&self.ship_memo, bundle, &who, |pending| self.offer(pending))`.
- **`offer`:**
  1. Mint `id = format!("skills_{}", Ulid::new())` and register with a drop guard.
  2. Send `SkillsOffer`.
  3. Await `Need` within `ARTIFACT_PULL_IDLE_TIMEOUT`. An `error` is `Err(Invalid)`. A ref that wasn't offered is an error.
  4. Send each needed skill's bytes as `ARTIFACT_CHUNK_BYTES` chunks (`seq` from 0, `last` on the final one; an empty tar is one empty `last` chunk). Each send is wrapped in the idle timeout.
  5. Await `Stored` and require it to list exactly the needed refs. An `error` is `Err(Invalid("host {id}: {error}"))`.
  6. Disconnect (`None`) is `Err(Unreachable)`.
  7. Return `Transfer::Done`.
- **`launch_agent`:** remove `refuse_bundle_launch`. A request with a bundle to a node without the capability is `Err(Unsupported)`, before any mirror run or frame. Otherwise set `RunSpec.skill_bundle`.

- [ ] **Step 1: Write failing tests:**
  - **unit tests** (`setup_with_capabilities` plus `route_offer`):
    - a capable node's offer → need → chunks → stored gives `Shipped` with the full manifest, and the chunks reassemble to the packed bytes;
    - 20 concurrent ships send one `SkillsOffer`;
    - a second ship sends none;
    - a node without the capability gives `Unsupported` with no frames sent, and a later ship after it "reconnects" with the capability ships (no memoised downgrade);
    - `close_pulls` in mid-offer gives `Unreachable`, and a retry offers again;
    - a `Stored` missing a ref is an error;
    - `launch_agent` with a bundle: to a capable node the `Run` frame's spec carries it; to an incapable node it is refused and nothing is sent.
  - **integration test** (`node_tunnel.rs`, real CP router plus a test-played node over the WebSocket, `connect_pull_capable_node` with `agent.skill_bundle` added): a full offer/need/chunk/stored exchange at real chunk size (a skill over 1 MiB, so it takes two chunks).
- [ ] **Step 2: Run them and confirm they fail.**
- [ ] **Step 3: Implement.** Route the new inbound frames in `server.rs`'s read pump next to `ArtifactChunk`.
- [ ] **Step 4: Run `cargo test -p rupu-cp --lib`, `cargo test -p rupu-cp --test it node_tunnel::` and clippy.**
- [ ] **Step 5: Commit** `feat(cp): ship a unit's skills to a tunnel node`.

---

### Task 4: Bucket port and connector

**Files:**
- Modify: `crates/rupu-cp/src/host/bucket/mod.rs` (the `Bucket` trait and `key_skill`), `object_store_bucket.rs`, `connector.rs`
- Modify: the other `Bucket` impls: `FailingBucket` (connector.rs ~:1051), `FinishesMidPoll` (poller.rs ~:188), `RecordingBucket` (rupu-cli node.rs ~:2188), `FlakyJobBucket` (node.rs ~:2730)
- Modify: `crates/rupu-cp/src/host/skill_ship.rs` (delete `not_built` and `refuse_bundle_launch`)
- Test: the connector.rs and object_store_bucket.rs unit tests, and `crates/rupu-cp/tests/it/bucket_e2e.rs`

**Interfaces:**
- Produces:
  ```rust
  // trait Bucket (required, no default — every transport says how)
  async fn put_skill(&self, hash: &str, bytes: &[u8]) -> Result<(), BucketError>;            // create-if-absent: an existing object is Ok
  async fn get_skill(&self, hash: &str, max_bytes: u64) -> Result<Vec<u8>, BucketError>;     // NotFound if absent; Io past max_bytes (checked by head before reading)
  pub(crate) fn key_skill(hash: &str) -> String;                                              // "skills/{hash}.tar"
  ```
  Use `PutOptions { mode: PutMode::Create }` for `put_skill`, mapping AlreadyExists to `Ok`.
- **`BucketHostConnector`** gains a `ship_memo: ShipMemo`.
  - **`ship_skills`:**
    1. Empty → `NotNeeded`.
    2. `require_worker_capability(CAP_AGENT_SKILL_BUNDLE, "shipped skills")`. `Err(Unsupported(m))` → `Ok(ShipOutcome::Unsupported(m))`, checked before `ship_through` and not memoised.
    3. Otherwise `ship_through(.., |pending| put each with put_skill)` → `Transfer::Done`.
    4. A bucket error is `Err(Unreachable("host {id}: …"))`.
  - **`launch_agent`:** remove `refuse_bundle_launch`. A bundle requires the capability (`require_worker_capability`), refused before the mirror run or job exists. Otherwise set `RunSpec.skill_bundle`.

- [ ] **Step 1: Write failing tests:**
  - object_store: `put_skill` twice is `Ok` and stores once; `get_skill` round-trips; it is `NotFound` when absent and refuses over `max_bytes`;
  - connector:
    - with workers advertising the capability, `ship_skills` puts `skills/<hash>.tar` for each skill and returns `Shipped`; a second ship puts nothing;
    - with a worker lacking it → `Unsupported`, and nothing is put;
    - `launch_agent` with a bundle writes a job whose `RunSpec` carries it, or is refused for an incapable pool.
- [ ] **Step 2: Run them and confirm they fail.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run `cargo test -p rupu-cp --lib bucket`, `cargo test -p rupu-cp --test it bucket_e2e::` and clippy.**
- [ ] **Step 5: Commit** `feat(cp): ship a unit's skills through a bucket`.

---

### Task 5: The bucket worker downloads skills

**Files:**
- Modify: `crates/rupu-cli/src/cmd/node.rs` (`bucket_tick` → `claim_new_jobs` → `start_claimed_job`, ~:1582–:1700). Thread `global` through.
- Test: the node.rs bucket tests (`tick`, `agent_job`, `FlakyJobBucket`, ~:2700–:3000)

**Interfaces:**
- Consumes: `Bucket::get_skill` (Task 4), `ship::{cache_dir, missing, store}`.
- Produces: `start_claimed_job` behaves as follows, after parsing the `RunSpec` and before `spawn_run`, when `spec.skill_bundle` is `Some`:
  1. Validate the manifest. An invalid one fails the job.
  2. For each ref `missing(cache_dir(global), …)` reports, `get_skill(hash, MAX_SHIPPED_SKILL_BYTES + 64 KiB)`, then `spawn_blocking(store)`.
  3. A transient bucket error → `pending.unfetched` (retried next tick, as a transient `get_job` error is today).
  4. `NotFound`, over-cap, or a store/verify error → the job is marked failed with a message naming `layer:name`.
  5. The run starts only when every manifest entry is cached.
- `bucket_worker_info` advertises the capability (from Task 1's list).

- [ ] **Step 1: Write failing tests:**
  1. a job with a bundle whose `skills/<hash>.tar` exists stores it in `global/cache/skills/<hash>/<name>` and spawns `rupu run … --skill-bundle=…`;
  2. an already-cached skill is not downloaded (count the `get_skill` calls);
  3. a missing object fails the job naming `layer:name`;
  4. a tampered object fails it and caches nothing;
  5. a transient `get_skill` error leaves the job pending, and the next tick starts it.
- [ ] **Step 2: Run them and confirm they fail.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run `cargo test -p rupu-cli --lib node`, `cargo test -p rupu-cp --test it bucket_e2e::` (extend `bucket_dead_drop_e2e` with one skill end to end: connector ships, worker downloads, the run argv carries `--skill-bundle`) and clippy.**
- [ ] **Step 5: Commit** `feat(node): bucket workers download shipped skills`.

---

### Task 6: Docs

**Files:** `docs/skills.md` (placed units: tunnel and bucket now ship; remove them from the known limits; the downgrade note for an old node or worker; that a capability downgrade on tunnel and bucket is re-checked per unit), `docs/cp-api.md` (only if an endpoint changed; none is planned), the spec's §4 (a note on the ref-carrying frames), `CLAUDE.md` (the rupu-cp entry: tunnel frames, the bucket `skills/` objects and worker download; the Read-first line marks Plan 2 complete), `docs/transcript-schema.md` (no change expected; check it).

- [ ] **Step 1:** Update each file against the code from Tasks 1–5.
- [ ] **Step 2:** Commit `docs(skills): tunnel and bucket hosts receive shipped skills`.

---

## Verification before the PR

```bash
cargo test -p rupu-cp --lib
cargo test -p rupu-cp --test it
cargo test -p rupu-cli --lib
cargo test -p rupu-cli --test it
cargo test -p rupu-skills
cargo clippy -p rupu-cp -p rupu-cli -p rupu-skills --all-targets
```

A live check against a real tunnel node or bucket worker is optional, and only with matt's go-ahead.
