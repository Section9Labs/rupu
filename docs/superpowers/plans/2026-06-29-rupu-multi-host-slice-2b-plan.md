# Multi-host Slice 2b — pull/bucket (dead-drop) transport Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a fifth `HostConnector` transport — a pull/bucket dead-drop — so a host that can't reach the CP at all (and vice-versa) can run dispatched work via a shared third-party object-store bucket.

**Architecture:** CP and node never connect; both talk only to a bucket (object store). `BucketHostConnector` writes job/control envelopes into the bucket via a `Bucket` port; a `rupu node pull` agent polls the bucket, atomically claims a job (`PutMode::Create`), runs it locally, and writes result envelopes back; a CP-side poller mirrors those into the central `RunStore` (reusing `NodeMirror` + the 2c shared mirror-backed observation). One `object_store`-backed `Bucket` impl serves S3/GCS/local; in-memory/local backends make it CI-testable.

**Tech Stack:** Rust 2021, tokio, the `object_store` crate (the one new dep), serde, async-trait, thiserror (libs) / anyhow (CLI).

## Global Constraints

- `object_store` is the ONE new workspace dependency — pinned in the ROOT `Cargo.toml` `[workspace.dependencies]` (with `features = ["aws", "gcp"]`; `memory`/`local` backends need no feature), referenced as `{ workspace = true }` from crate manifests. No other new deps. No version literals in crate `Cargo.toml`.
- **The `object_store` API has version drift.** The code in this plan targets a recent API (`parse_url_opts`, `PutMode::Create`, `put_opts`, `PutPayload`, `list`, `get().bytes()`). The T1 implementer MUST confirm the EXACT call shapes against the pinned version and adapt (e.g. `PutPayload` vs `Bytes`, `PutOptions` construction, the already-exists error variant name). This is "use the real API of the pinned dep," not a placeholder.
- `#![deny(clippy::all)]` workspace-wide; `unsafe_code` forbidden (no `unsafe`).
- Library errors `thiserror`; CLI binary `anyhow`.
- Per-file `rustfmt` only — `main` is fmt-dirty; NEVER workspace-wide `cargo fmt`.
- `rupu-cli` has a PRE-EXISTING red toolchain baseline; verify only NEW code compiles + its tests pass.
- **No secrets in rupu** — the `Bucket` host record holds only `url` + `prefix`; credentials come from the env / cloud credential chain (`object_store` resolves them). No `token_hash`.
- Branch `worktree-multi-host-slice-2b` is STACKED on 2c (PR #419) → 2.5 (#418); it reuses the 2c shared `mirror_list_runs`/`mirror_get_run`/`mirror_stream_run_events` helpers + `NodeMirror`, and extends the resume-worker `remote_workers` filter (now Tunnel+Ssh) to Bucket. Those changes are present on this branch.
- Atomic claim via `PutMode::Create` guarantees at-most-one node runs a job.

---

## File Structure

- `Cargo.toml` (root) — add `object_store` to `[workspace.dependencies]`. (T1)
- `crates/rupu-cp/src/host/bucket/mod.rs` — `Bucket` trait, `BucketError`, envelope key helpers. (T1)
- `crates/rupu-cp/src/host/bucket/object_store_bucket.rs` — `ObjectStoreBucket` impl + tests (in-memory/local). (T1)
- `crates/rupu-cp/src/host/bucket/connector.rs` — `BucketHostConnector`. (T3)
- `crates/rupu-cp/src/host/mod.rs` — `pub mod bucket;`. (T1)
- `crates/rupu-cp/Cargo.toml` — `object_store = { workspace = true }`. (T1)
- `crates/rupu-workspace/src/host_store.rs` — `HostTransport::Bucket` + `add_bucket_host`. (T2)
- exhaustive `HostTransport` match arms: `crates/rupu-cli/src/cmd/host.rs`, `crates/rupu-cp/src/api/hosts.rs`, `crates/rupu-cp/src/host/registry.rs`. (T2, T4)
- `crates/rupu-cli/src/cmd/cp.rs` — CP-side bucket poller. (T5)
- `crates/rupu-cli/src/cmd/node.rs` + command enum — `rupu node pull`. (T6)
- `crates/rupu-cli/src/cmd/cp.rs` — resume-worker filter. (T7)
- `crates/rupu-cp/tests/bucket_e2e.rs` — e2e. (T8)

---

## Task 1: `object_store` dep + `Bucket` port + `ObjectStoreBucket`

**Files:**
- Modify: root `Cargo.toml`; `crates/rupu-cp/Cargo.toml`; `crates/rupu-cp/src/host/mod.rs`
- Create: `crates/rupu-cp/src/host/bucket/mod.rs`, `crates/rupu-cp/src/host/bucket/object_store_bucket.rs`

**Interfaces — Produces:**
```rust
#[async_trait::async_trait]
pub(crate) trait Bucket: Send + Sync {
    async fn put_job(&self, run_id: &str, envelope: &[u8]) -> Result<(), BucketError>;
    async fn list_jobs(&self) -> Result<Vec<String>, BucketError>;          // run_ids with a job and (maybe) no claim
    async fn claim_job(&self, run_id: &str, worker: &str) -> Result<bool, BucketError>; // PutMode::Create; Ok(false) if already claimed
    async fn get_job(&self, run_id: &str) -> Result<Vec<u8>, BucketError>;
    async fn put_control(&self, run_id: &str, seq: u64, envelope: &[u8]) -> Result<(), BucketError>;
    async fn list_control(&self, run_id: &str) -> Result<Vec<(u64, Vec<u8>)>, BucketError>; // sorted by seq
    async fn put_result(&self, run_id: &str, key: &str, body: &[u8]) -> Result<(), BucketError>;
    async fn list_results(&self, run_id: &str) -> Result<Vec<(String, Vec<u8>)>, BucketError>; // sorted by key
    async fn put_finished(&self, run_id: &str, status: &str) -> Result<(), BucketError>;
    async fn get_finished(&self, run_id: &str) -> Result<Option<String>, BucketError>;
    async fn probe(&self) -> Result<(), BucketError>;
}
pub(crate) struct ObjectStoreBucket { /* store: Arc<dyn ObjectStore>, prefix: object_store::path::Path */ }
impl ObjectStoreBucket {
    pub(crate) fn new(store: std::sync::Arc<dyn object_store::ObjectStore>, prefix: &str) -> Self;     // tests pass InMemory/LocalFileSystem
    pub(crate) fn from_url(url: &str, prefix: Option<&str>) -> Result<Self, BucketError>;              // prod: object_store::parse_url_opts
}
#[derive(Debug, thiserror::Error)] pub(crate) enum BucketError { #[error("bucket io: {0}")] Io(String), #[error("not found: {0}")] NotFound(String) }
```

- [ ] **Step 1: Add the dependency**

Add to root `Cargo.toml` `[workspace.dependencies]` (confirm the latest stable version with `cargo search object_store` or crates.io; pin it):
```toml
object_store = { version = "0.11", features = ["aws", "gcp"] }
```
Add to `crates/rupu-cp/Cargo.toml` `[dependencies]`: `object_store = { workspace = true }`. (Confirm `futures-util`/`bytes` are already deps — `object_store` payloads use `bytes::Bytes`; if `bytes` isn't a workspace dep yet, add it: `bytes = "1"` in root, `{ workspace = true }` in the crate.) Add `pub mod bucket;` to `crates/rupu-cp/src/host/mod.rs`.

- [ ] **Step 2: Write failing Bucket tests (CI via in-memory backend)**

In `crates/rupu-cp/src/host/bucket/object_store_bucket.rs` `#[cfg(test)] mod tests`:

```rust
fn mem_bucket() -> ObjectStoreBucket {
    ObjectStoreBucket::new(std::sync::Arc::new(object_store::memory::InMemory::new()), "test-prefix/host_1")
}

#[tokio::test]
async fn job_put_list_get_roundtrip() {
    let b = mem_bucket();
    b.put_job("run_1", br#"{"kind":"workflow"}"#).await.unwrap();
    assert_eq!(b.list_jobs().await.unwrap(), vec!["run_1".to_string()]);
    assert_eq!(b.get_job("run_1").await.unwrap(), br#"{"kind":"workflow"}"#);
}

#[tokio::test]
async fn claim_is_atomic_once() {
    let b = mem_bucket();
    b.put_job("run_1", b"{}").await.unwrap();
    assert!(b.claim_job("run_1", "node-a").await.unwrap());      // first wins
    assert!(!b.claim_job("run_1", "node-b").await.unwrap());     // second loses (already claimed)
}

#[tokio::test]
async fn control_and_results_ordered_by_seq_key() {
    let b = mem_bucket();
    b.put_control("run_1", 2, b"c2").await.unwrap();
    b.put_control("run_1", 1, b"c1").await.unwrap();
    let ctl = b.list_control("run_1").await.unwrap();
    assert_eq!(ctl.iter().map(|(s,_)| *s).collect::<Vec<_>>(), vec![1, 2]);
    b.put_result("run_1", "events.0001.jsonl", b"line").await.unwrap();
    assert_eq!(b.list_results("run_1").await.unwrap().len(), 1);
}

#[tokio::test]
async fn finished_marker_roundtrip() {
    let b = mem_bucket();
    assert_eq!(b.get_finished("run_1").await.unwrap(), None);
    b.put_finished("run_1", "completed").await.unwrap();
    assert_eq!(b.get_finished("run_1").await.unwrap().as_deref(), Some("completed"));
}
```

- [ ] **Step 3: Run to verify they fail**

Run: `cargo test -p rupu-cp --lib host::bucket`
Expected: FAIL — module/types not defined.

- [ ] **Step 4: Implement `Bucket` + `ObjectStoreBucket`**

Create `crates/rupu-cp/src/host/bucket/mod.rs` with the trait + `BucketError` + a `pub(crate) mod object_store_bucket; pub(crate) use object_store_bucket::ObjectStoreBucket;` and the key-layout helpers (functions building the object paths under the prefix: `jobs/<id>.json`, `jobs/<id>.claim`, `control/<id>/<seq:020>.json`, `runs/<id>/<key>`, `runs/<id>/finished`). Use zero-padded seq (`format!("{seq:020}")`) so lexical list order = numeric order.

Create `crates/rupu-cp/src/host/bucket/object_store_bucket.rs` implementing the trait against `object_store::ObjectStore`. Key calls (ADAPT to the pinned version's exact API — verify each):
- construct path: `self.prefix.child("jobs").child(format!("{run_id}.json"))` (use `object_store::path::Path`).
- `put_job`/`put_control`/`put_result`/`put_finished`: `self.store.put(&path, bytes::Bytes::copy_from_slice(body).into()).await` (the payload type may be `PutPayload`; `.into()` from `Bytes`).
- `claim_job`: `self.store.put_opts(&claim_path, payload, object_store::PutOptions { mode: object_store::PutMode::Create, ..Default::default() }).await` → `Ok(_) => Ok(true)`, `Err(object_store::Error::AlreadyExists { .. }) => Ok(false)`, other `Err(e) => Err(BucketError::Io(e.to_string()))`.
- `list_jobs`/`list_control`/`list_results`: `self.store.list(Some(&dir_prefix))` then collect via `futures_util::TryStreamExt::try_collect` into `Vec<ObjectMeta>`; map `.location` to the run_id / (seq, body) / (key, body). For `list_jobs`, list `jobs/`, take `*.json` entries, and EXCLUDE ids that already have a `*.claim` (so it returns unclaimed jobs). For control/results, `get` each object's bytes.
- `get_job`/`get_finished`: `self.store.get(&path).await` → `.bytes().await`; map a not-found error to `NotFound`/`Ok(None)`.
- `probe`: a cheap `self.store.list_with_delimiter(Some(&self.prefix)).await` or a `head` on a sentinel — pick a call that succeeds on a reachable bucket and errors on an unreachable/misconfigured one.
- `from_url`: `let (store, path) = object_store::parse_url_opts(&url::Url::parse(url)?, std::iter::empty())?;` then prefix = `path` joined with the optional `prefix`. (Confirm `parse_url_opts` signature; `url` crate may already be a dep — if not, `object_store` re-exports a parser, use that.)

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p rupu-cp --lib host::bucket`
Expected: PASS (4 tests).
Run: `cargo clippy -p rupu-cp`
Expected: clean.

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml crates/rupu-cp/Cargo.toml crates/rupu-cp/src/host/mod.rs crates/rupu-cp/src/host/bucket/
git commit -m "feat(cp): Bucket port + object_store-backed impl (atomic claim)"
```

---

## Task 2: `HostTransport::Bucket` + `add_bucket_host`

**Files:**
- Modify: `crates/rupu-workspace/src/host_store.rs` (+ lib.rs re-export); keep-green arms in `crates/rupu-cli/src/cmd/host.rs`, `crates/rupu-cp/src/api/hosts.rs`, `crates/rupu-cp/src/host/registry.rs`.

This task is a direct analog of Slice 2c Task 1 (which added `HostTransport::Ssh`). Read the committed `Ssh` variant + `add_ssh_host` + the three match arms it added, and mirror them for `Bucket`.

**Interfaces — Produces:** `HostTransport::Bucket { url: String, prefix: Option<String> }` (serde `kind = "bucket"`, optional `prefix` with `#[serde(default, skip_serializing_if = "Option::is_none")]`); `add_bucket_host(store, name, url, prefix) -> Result<Host, HostStoreError>` (assigns `host_<ULID>`, no `token_hash`).

- [ ] **Step 1: Failing tests** — in `host_store.rs` tests, add `bucket_transport_serde_roundtrip` (assert `kind = "bucket"` + `url` present + round-trip equality; with and without `prefix`) and `add_bucket_host_persists_no_token` (id prefix `host_`, transport is Bucket, `token_hash` None, reloads). Model them on the `Ssh` equivalents.

- [ ] **Step 2: Run** `cargo test -p rupu-workspace bucket_transport_serde_roundtrip add_bucket_host_persists_no_token` → FAIL.

- [ ] **Step 3: Add the variant + helper.** In `HostTransport` (after `Ssh`):
```rust
    /// Reachable only via a shared object-store bucket (dead-drop). The CP and
    /// the node never connect; both talk to the bucket. `url` is an
    /// object_store URL (`s3://…` / `gs://…` / `file://…`); credentials come
    /// from the environment / cloud credential chain, never stored here.
    Bucket {
        url: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        prefix: Option<String>,
    },
```
Add `add_bucket_host` mirroring `add_ssh_host` (same id/timestamp helpers, `token_hash: None`). Re-export it from lib.rs alongside `add_ssh_host`.

- [ ] **Step 4: Keep-green arms** (mirror the `Ssh` arms):
1. `cmd/host.rs` list display: `HostTransport::Bucket { url, .. } => format!("bucket:{url}")`.
2. `api/hosts.rs` transport_fields: kind `"bucket"`, address field = the `url` (never a secret).
3. `host/registry.rs` `build_connector`: TEMPORARY `HostTransport::Bucket { .. } => Err(HostConnectorError::Invalid("bucket transport not yet wired (slice 2b task 4)".into()))` (real arm in Task 4).

- [ ] **Step 5: Verify** `cargo test -p rupu-workspace bucket_transport_serde_roundtrip add_bucket_host_persists_no_token` → PASS; `cargo build -p rupu-workspace -p rupu-cp -p rupu-cli` → compiles; `cargo clippy -p rupu-workspace -p rupu-cp` → clean.

- [ ] **Step 6: Commit**
```bash
git add crates/rupu-workspace/src/host_store.rs crates/rupu-workspace/src/lib.rs crates/rupu-cli/src/cmd/host.rs crates/rupu-cp/src/api/hosts.rs crates/rupu-cp/src/host/registry.rs
git commit -m "feat(workspace): HostTransport::Bucket + add_bucket_host"
```

---

## Task 3: `BucketHostConnector`

**Files:**
- Create: `crates/rupu-cp/src/host/bucket/connector.rs`; add `pub(crate) mod connector; pub(crate) use connector::BucketHostConnector;` to `host/bucket/mod.rs`.
- Test: same file `#[cfg(test)]`.

This is a direct analog of `SshHostConnector` (Slice 2c) but the transport is the `Bucket` port instead of `RemoteExec`. Read `crates/rupu-cp/src/host/ssh.rs`'s connector for the exact `use` block, the `HostConnector` trait method set, the RunSpec construction in `launch_run`/`launch_agent`, and how observation delegates to the shared `mirror_*` helpers.

**Interfaces — Produces:** `pub(crate) struct BucketHostConnector { host_id: String, bucket: Arc<dyn Bucket>, mirror: Arc<NodeMirror>, run_store: Arc<RunStore>, pricing: PricingConfig }` + `new(...)`.

- [ ] **Step 1: Failing tests (via the in-memory bucket).** Build a connector over `ObjectStoreBucket::new(InMemory, prefix)` and a temp `RunStore`/`NodeMirror`. Assert:
  - `launch_run` mints `run_<ULID>`, `mirror.create_run` records it (worker_id == host_id), and a job envelope is in the bucket (`bucket.get_job(run_id)` returns the RunSpec JSON containing `"workflow"`/the name).
  - `cancel_run` / `approve_run("bypass")` / `reject_run(Some("nope"))` each write a `control/<id>/<seq>` envelope (`bucket.list_control(run_id)` returns the right kind/payload).
  - `info().reachable == true` for a healthy in-memory bucket.

- [ ] **Step 2: Run** `cargo test -p rupu-cp --lib host::bucket::connector` → FAIL.

- [ ] **Step 3: Implement.** `launch_run`/`launch_agent`: mint id → build `RunSpec` (Workflow/Agent, name, inputs/prompt/mode/target — copy the SshHostConnector construction) → `serde_json::to_vec(&spec)` → `mirror.create_run(&run_id, &self.host_id, &spec)` → `bucket.put_job(&run_id, &bytes).await` (map `BucketError`→`HostConnectorError::Unreachable`) → return id. Control: a private `put_control_envelope(run_id, ControlKind)` that allocates the next seq (count existing control objects via `list_control` len, or a monotonic based on it) and writes a JSON envelope `{ "kind": "cancel"|"approve"|"reject", "mode"?, "reason"? }`; `cancel_run`/`approve_run`/`reject_run` call it. Observation: delegate to `crate::host::connector::mirror_list_runs/mirror_get_run/mirror_stream_run_events` with `&self.host_id`; `get_transcript` → `read_transcript_file`; `proxy_get_json` → `Invalid`; `start_session`/`send_session_turn` → `Invalid("sessions not supported over bucket (slice 2b)")`. `info()` → `reachable = self.bucket.probe().await.is_ok()`.

  Define the control envelope as a serde struct shared with the node side — put `ControlEnvelope { kind: String, mode: Option<String>, reason: Option<String> }` and the job/result envelope shapes in `host/bucket/mod.rs` so both the connector (T3) and the node agent (T6) use the SAME types (no drift). The job envelope is the existing `crate::node::protocol::RunSpec` (reuse it; do not re-declare).

- [ ] **Step 4: Run** `cargo test -p rupu-cp --lib host::bucket::connector` → PASS; `cargo clippy -p rupu-cp` → clean.

- [ ] **Step 5: Commit**
```bash
git add crates/rupu-cp/src/host/bucket/
git commit -m "feat(cp): BucketHostConnector (dispatch + queued control + mirror observation)"
```

---

## Task 4: Registry resolution — `Bucket` arm

**Files:** Modify `crates/rupu-cp/src/host/registry.rs` (replace the Task-2 placeholder); test in `crates/rupu-cp/tests/host_registry.rs`.

Direct analog of the Slice 2c `Ssh` arm. Read it and mirror.

- [ ] **Step 1: Failing resolution test** — mirror `ssh_host_resolves_*`: build a registry `.with_tunnel_deps(...)` (supplies mirror+run_store+pricing), `add_bucket_host` (use a `file://<tempdir>` url so it's constructible without cloud), `resolve(&host.id)` → `Ok`, `info().await` → `Ok` (reachable true for a real temp-dir file bucket, or just assert `Ok`). Negative: a registry WITHOUT deps → `Invalid`.

- [ ] **Step 2: Run** `cargo test -p rupu-cp bucket_host_resolves` → FAIL (placeholder Invalid).

- [ ] **Step 3: Implement the arm:**
```rust
            HostTransport::Bucket { url, prefix } => {
                match (&self.node_mirror, &self.run_store) {
                    (Some(mir), Some(store)) => {
                        let bucket = crate::host::bucket::ObjectStoreBucket::from_url(url, prefix.as_deref())
                            .map_err(|e| HostConnectorError::Invalid(format!("bad bucket url: {e}")))?;
                        Ok(std::sync::Arc::new(crate::host::bucket::BucketHostConnector::new(
                            host.id.clone(),
                            std::sync::Arc::new(bucket),
                            std::sync::Arc::clone(mir),
                            std::sync::Arc::clone(store),
                            self.pricing.clone(),
                        )))
                    }
                    _ => Err(HostConnectorError::Invalid(
                        "bucket deps not wired (call HostRegistry::with_tunnel_deps; mirror + run_store shared with tunnel)".into())),
                }
            }
```
Make the bucket items `pub(crate)` as needed.

- [ ] **Step 4: Run** `cargo test -p rupu-cp bucket_host_resolves` → PASS; `cargo clippy -p rupu-cp` → clean.

- [ ] **Step 5: Commit**
```bash
git add crates/rupu-cp/src/host/registry.rs crates/rupu-cp/src/host/bucket/ crates/rupu-cp/tests/host_registry.rs
git commit -m "feat(cp): resolve Bucket transport to BucketHostConnector"
```

---

## Task 5: CP-side bucket poller

**Files:** Modify `crates/rupu-cli/src/cmd/cp.rs` (add a `run_bucket_poller` task spawned beside the resume worker at ~line 56).

**Interfaces — Consumes:** `rupu_workspace::HostStore` (to discover `Bucket` hosts), `ObjectStoreBucket::from_url`, the `Bucket` trait, `NodeMirror` (build one over the same `RunStore`), `ArtifactFile`.

- [ ] **Step 1: Write the poller test** (in `crates/rupu-cp/tests/bucket_e2e.rs` — created here, shared with T8). Seed an in-memory/`file://` bucket with a created mirror run + result objects (`runs/<id>/events.0001.jsonl`, a `run.json`, and a `finished`=completed marker), then run ONE poll pass of an extracted `poll_bucket_run(bucket, mirror, host_id, run_id, &mut consumed) ` helper and assert: the event line is appended to the central run's events.jsonl, run.json mirrored, and the run is `Completed`. Re-running the pass does NOT double-append (idempotent via the `consumed` set). Extract the per-run poll logic into a `pub(crate)` testable fn so the test doesn't need the full `cp serve` loop.

- [ ] **Step 2: Run** → FAIL (fn not defined).

- [ ] **Step 3: Implement.** Add to `cp.rs`:
  - `async fn poll_bucket_run(bucket: &dyn Bucket, mirror: &NodeMirror, host_id: &str, run_id: &str, consumed: &mut HashSet<String>) -> anyhow::Result<bool>`: `list_results(run_id)` → for each `(key, body)` not in `consumed`: route by key suffix to `ArtifactFile` (`events`→Events, `step_results`→StepResults, `unit_checkpoints`→UnitCheckpoints, `run.json`→RunJson) and `mirror.append(run_id, host_id, file, line)` per line (split body on `\n`); insert key into `consumed`. Then `get_finished(run_id)` → if `Some(status)`, `mirror.finish(run_id, host_id, &status)` and return `true` (run done). Else `false`.
  - `async fn run_bucket_poller(store: Arc<RunStore>, hosts: HostStore, mut shutdown: watch::Receiver<bool>)`: loop on an interval (e.g. `BUCKET_POLL_INTERVAL = 15s`, with shutdown select like the resume worker): for each `Bucket` host in `hosts.list()`, build `ObjectStoreBucket::from_url` + a `NodeMirror::new(store.clone())`; discover the host's in-flight run_ids from the RunStore (runs with `worker_id == host.id` and non-terminal status — reuse `store` queries) and call `poll_bucket_run` for each, maintaining a per-`(host,run)` `consumed` set across iterations (a `HashMap<String, HashSet<String>>` in the task). On a run returning done, best-effort clean its bucket keys (optional; note if deferred). Don't hold any lock across `.await`.
  - Spawn it in `cp serve` next to the resume worker (same `global_dir`/`HostStore`/shutdown pattern), and await its handle on shutdown.

- [ ] **Step 4: Run** the poller test → PASS; `cargo build -p rupu-cli` compiles; `cargo clippy -p rupu-cp` clean.

- [ ] **Step 5: Commit**
```bash
git add crates/rupu-cli/src/cmd/cp.rs crates/rupu-cp/tests/bucket_e2e.rs
git commit -m "feat(cp): bucket poller mirrors dead-drop results into RunStore"
```

---

## Task 6: `rupu node pull` agent

**Files:** Modify `crates/rupu-cli/src/cmd/node.rs` + the command enum/dispatch (`crates/rupu-cli/src/lib.rs`).

**Interfaces — Consumes:** `ObjectStoreBucket`, the `Bucket` trait, the existing node helpers `build_argv`/`spawn_run`/`build_control_argv`/`spawn_control`/`drain_new_lines`/`RunState` (all in node.rs), `RunSpec`, the shared `ControlEnvelope` (T3).

- [ ] **Step 1: Write the unit-testable claim/drain test.** The full loop spawns subprocesses; extract the pure-ish pieces and test them: (a) `claim_job` atomicity is already covered (T1); (b) add a test for a `fn classify_result_key(key) -> Option<ArtifactFile>` helper (the node uses the same key→file mapping when WRITING results) and a `fn next_control_seq(existing: &[(u64, Vec<u8>)]) -> u64` helper. Keep the subprocess loop out of unit tests (T8 e2e covers the dispatch path with a simulated node step).

- [ ] **Step 2: Run** → FAIL.

- [ ] **Step 3: Implement `rupu node pull`.** Add a `Pull(PullArgs)` variant to the node subcommand enum: `--bucket <url>`, `--prefix <p>`, `--host-id <id>` (defaults to a stable id like the existing node id resolution), `--once` (bool), `--interval <secs>` (default 15). Handler `async fn pull(args)`:
  - Build `ObjectStoreBucket::from_url(&args.bucket, args.prefix.as_deref())`.
  - Loop (or once if `--once`): `list_jobs()` → for each run_id: `claim_job(run_id, host_id)`; if `false`, skip. If won: `get_job` → deserialize `RunSpec` → `spawn_run(exe, &run_id, &spec)` (reuse) → register in an `active: HashMap<String, RunState>`.
  - For each active run each tick: `drain_new_lines` on its `<runs_root>/<run_id>/{events,step_results,unit_checkpoints}.jsonl` (reuse the offsets in `RunState`) and `bucket.put_result(run_id, &format!("events.{seq:04}.jsonl"), joined_lines)` (one result object per non-empty drain, monotonic seq per file kind); read `run.json` and `put_result(run_id, "run.json", body)`; drain `list_control(run_id)` beyond the last-applied seq → cancel: `state.child.start_kill()`; approve/reject: `spawn_control` with `build_control_argv` (reuse). On terminal `run.json` status: `put_finished(run_id, status)`, remove from `active`.
  - `--once`: after draining all currently-claimable jobs AND letting active runs reach terminal (bounded), exit. Loop mode: sleep `interval` between ticks.
  - The runs root is the same `paths::global_dir().join("runs")` the existing node agent uses — reuse that resolution.
  - Bucket auth uses the env cred chain (no rupu secret). `anyhow` errors.

- [ ] **Step 4: Run** `cargo test -p rupu-cli --lib node` (the helper tests + existing tests pass); `cargo build -p rupu-cli` compiles.

- [ ] **Step 5: Commit**
```bash
git add crates/rupu-cli/src/cmd/node.rs crates/rupu-cli/src/lib.rs
git commit -m "feat(cli): rupu node pull (bucket dead-drop agent)"
```

---

## Task 7: Resume-worker filter — skip Bucket hosts

**Files:** Modify `crates/rupu-cli/src/cmd/cp.rs` (the `remote_workers` filter from 2.5/2c); test in `crates/rupu-cp/tests/node_tunnel.rs`.

- [ ] **Step 1: Failing invariant test** — mirror the 2c `mirrored_awaiting_run_is_not_pending_resume_ssh_host` test for a `worker_id = Some("host_bucket_1")` awaiting run with no marker → not in `list_pending_resume`.

- [ ] **Step 2: Run** → PASS immediately (primary invariant: no marker → not listed). Then extend the filter.

- [ ] **Step 3: Extend the `remote_workers` filter_map** to add `rupu_workspace::HostTransport::Bucket { .. } => Some(h.id),` alongside the Tunnel/Ssh arms.

- [ ] **Step 4: Verify** `cargo build -p rupu-cli` compiles; `cargo test -p rupu-cp --test node_tunnel mirrored_awaiting` → PASS; `cargo clippy -p rupu-cp` clean.

- [ ] **Step 5: Commit**
```bash
git add crates/rupu-cli/src/cmd/cp.rs crates/rupu-cp/tests/node_tunnel.rs
git commit -m "fix(cp): resume worker skips bucket-host runs (defense-in-depth)"
```

---

## Task 8: e2e — dispatch → claim → result → mirror (in-memory bucket)

**Files:** Modify `crates/rupu-cp/tests/bucket_e2e.rs` (extend from T5).

- [ ] **Step 1: Write the e2e.** Using a `file://<tempdir>` (or shared `InMemory`) bucket and a temp `RunStore`/`NodeMirror`:
  1. Build a `BucketHostConnector` over the bucket; `launch_run` → assert a job envelope exists in the bucket and a mirror run is `Running`.
  2. Simulate the node step directly against the bucket (no subprocess): `claim_job` (assert true) → write `runs/<id>/events.0001.jsonl` (a valid `rupu_orchestrator::executor::Event` line), a `run.json` with `status:"completed"`, and `put_finished("completed")`.
  3. Run `poll_bucket_run` (T5) → assert the central run shows `Completed` and the event line is in its `events.jsonl` (via the shared observation / RunStore load).
  4. Control: `connector.cancel_run(run_id)` → assert a control envelope is in the bucket (`list_control`), proving queued delivery.

- [ ] **Step 2: Run** `cargo test -p rupu-cp --test bucket_e2e` → PASS. Then full `cargo test -p rupu-cp -p rupu-workspace` green + `cargo clippy -p rupu-cp -p rupu-workspace` clean.

- [ ] **Step 3: Commit**
```bash
git add crates/rupu-cp/tests/bucket_e2e.rs
git commit -m "test(cp): bucket dead-drop e2e — dispatch, claim, mirror, control"
```

---

## Self-Review

**Spec coverage:** Bucket port + object_store impl + atomic claim → T1. HostTransport::Bucket + enrollment → T2 (+ CLI flag folded into T2's keep-green + a `--bucket` add path; NOTE: ensure the `rupu host add --bucket` CLI flag is added — fold into T2 Step 4 alongside the list arm, mirroring 2c's `--ssh`). BucketHostConnector (dispatch + queued control + observation) → T3. Registry → T4. CP poller (mirror results) → T5. `rupu node pull` (loop default + --once + atomic claim + control apply) → T6. Resume filter → T7. e2e → T8. No secrets (creds from env) — T1/T2 (no token_hash; from_url uses the cred chain). Atomic claim → T1. Reuse NodeMirror + 2c shared observation → T3/T5.

**GAP found in self-review → FIX:** the CLI `rupu host add --bucket <url> [--prefix p]` enrollment flag must be added (the spec's enrollment goal). Add it in **Task 2 Step 4** (extend `AddArgs` with `--bucket`/`--prefix`, mutually exclusive with `--url`/`--ssh`, branch in `add_inner` to `add_bucket_host`) — mirroring how 2c added `--ssh`. Add a `bucket_add_roundtrip` test there.

**Placeholder scan:** the only deferred-to-pinned-version items are the `object_store` call shapes in T1, explicitly flagged as "adapt to the pinned API" (the dep is the source of truth) — not placeholders. The Task-2 `build_connector` Bucket placeholder is replaced in Task 4. No TBDs.

**Type consistency:** `Bucket` trait methods, `ObjectStoreBucket::{new, from_url}`, `BucketHostConnector { host_id, bucket, mirror, run_store, pricing }`, and the shared `ControlEnvelope`/`RunSpec` envelope types are used consistently across T1/T3/T4/T5/T6. `worker_id == host record id` for bucket runs is consistent across create_run (T3), poller (T5), and resume filter (T7).

---

## Process Note

Branch + single PR per repo convention. `worktree-multi-host-slice-2b` is STACKED on 2c (PR #419). Build subagent-driven (TDD). After all tasks: final whole-branch review (opus), then finishing-a-development-branch → push + PR (base = the 2c branch, retargets to main as the stack merges; no self-merge). object_store is the one new dep — the final review should sanity-check the dependency addition + that no credentials are persisted.
