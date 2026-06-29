# rupu multi-host — Slice 2b: pull/bucket (dead-drop) transport

Status: approved (design), pending implementation plan
Date: 2026-06-29

## Context

The `HostConnector` port now has four transports: `Local`, `HttpCp` (Slice 1 —
the host runs a reachable `rupu cp serve`), `Tunnel` (Slice 2 — node dials home
over a persistent WebSocket), and `Ssh` (Slice 2c — CP connects out over ssh).
All four require CP and node to reach **each other** somehow (inbound server,
held outbound WS, or outbound ssh).

Slice 2b adds the most decoupled transport: **pull/bucket (dead-drop)**, for the
case where CP and node **cannot reach each other at all**. Both independently
reach a third-party **bucket** (an object store — S3/GCS across the internet, or
a shared filesystem on a local network) that acts as the connecting gateway.
The CP writes dispatched work into the bucket; the node polls the bucket,
atomically claims a job, runs it locally, and writes results back; a CP-side
poller reads the results and mirrors them into the central `RunStore`. This is
the Okesu CP/Node dead-drop model.

This is sequential after Slice 2.5 (#418) and Slice 2c (#419) and **builds on
them**: it reuses the shared `NodeMirror` and the mirror-backed observation
helpers extracted in 2c, and extends the resume-worker filter (now covering
Tunnel + Ssh) to Bucket hosts. It ships as its own PR.

## Spine decisions (approved)

1. **Bucket = a third-party object store, behind a `Bucket` port.** One
   `object_store`-backed implementation serves S3 / GCS / Azure / local /
   in-memory via configuration. CP and node never connect — both only talk to
   the bucket.
2. **Backends now (single PR): object store via the `object_store` crate.** The
   crate's `memory` / `local` backends exercise the EXACT object-store code path
   in CI (no cloud, no credentials), so the one implementation is fully testable
   while S3/GCS work in production through the same code.
3. **Atomic claim = `PutMode::Create`** (create-if-absent) on a per-job claim
   marker object — the portable primitive across object_store backends; prevents
   two nodes running the same job.
4. **Credentials are NOT stored in rupu** — `object_store` reads them from the
   standard cloud credential chain / environment (`AWS_*`, `GOOGLE_*`), matching
   the ssh "no secrets" principle. A managed `rupu auth hosts` path is future
   work.
5. **`rupu node pull` loops by default** (always-on, polls on an interval);
   `--once` drains the bucket a single time and exits (cron/batch hosts).
6. **Control (cancel/approve/reject) is queued in the bucket** and applied on the
   node's next poll — inherently async (diverges from 2.5's direct-frame, which
   is correct for a dead-drop).

## Goals (Slice 2b)

- Enroll a bucket host: `rupu host add --bucket <url> [--prefix <p>] <name>` →
  `HostTransport::Bucket` host record (no secrets).
- From the central CP: launch a workflow or agent run on a bucket host (writes a
  job envelope to the bucket); a `rupu node pull` agent on the host claims and
  runs it; results stream back into the central mirror and appear in the existing
  host-aware lists / RunDetail / live events.
- Cancel / approve / reject a bucket-host run from the central CP (queued control
  envelope, applied on the node's next poll).
- Two nodes sharing a bucket never double-run a job (atomic claim).

## Non-goals (later)

- Real-time control / sub-poll-interval latency (pull is poll-bound).
- A web "Add bucket host" UI (CLI enrollment only, as for ssh).
- Managed credential storage in rupu (`rupu auth hosts` — future); creds come
  from the env / cloud chain.
- Interactive sessions over the bucket (sessions are Slice 5, all transports).
- Bucket garbage collection / retention policy beyond a basic
  consumed-marker cleanup (note as future).

## Architecture

Seven pieces; the `Bucket` port + the CP poller + the pull agent are the
substantial new code.

### 1. Dependency + `Bucket` port (`crates/rupu-cp/src/host/bucket/`)

Add `object_store` to the ROOT `[workspace.dependencies]` (enable the `aws` +
`gcp` features for S3/GCS; `memory`/`local` are always available). This is the
first new external dependency in the multi-host effort — justified: a unified,
well-maintained (Apache Arrow) object-store abstraction whose local/in-memory
backends make the prod S3/GCS path CI-testable.

Define a `Bucket` trait (the port):

```rust
#[async_trait]
pub(crate) trait Bucket: Send + Sync {
    async fn put_job(&self, run_id: &str, envelope: &[u8]) -> Result<(), BucketError>;
    async fn list_jobs(&self) -> Result<Vec<String>, BucketError>;           // unclaimed run_ids
    async fn claim_job(&self, run_id: &str, worker: &str) -> Result<bool, BucketError>; // PutMode::Create; false if already claimed
    async fn get_job(&self, run_id: &str) -> Result<Vec<u8>, BucketError>;
    async fn put_control(&self, run_id: &str, seq: u64, envelope: &[u8]) -> Result<(), BucketError>;
    async fn list_control(&self, run_id: &str) -> Result<Vec<(u64, Vec<u8>)>, BucketError>;
    async fn put_result(&self, run_id: &str, key: &str, body: &[u8]) -> Result<(), BucketError>;
    async fn list_results(&self, run_id: &str) -> Result<Vec<(String, Vec<u8>)>, BucketError>;
    async fn put_finished(&self, run_id: &str, status: &str) -> Result<(), BucketError>;
    async fn get_finished(&self, run_id: &str) -> Result<Option<String>, BucketError>;
    async fn probe(&self) -> Result<(), BucketError>;                        // reachability
}
```

One impl, `ObjectStoreBucket { store: Arc<dyn object_store::ObjectStore>, prefix: Path }`, constructed from the host's `url` via `object_store::parse_url_opts` (so `s3://`, `gs://`, `file://`, `memory://` all parse). `claim_job` uses `store.put_opts(claim_path, body, PutMode::Create.into())` and maps the already-exists error to `Ok(false)`.

### 2. Bucket layout (keys under `<prefix>/<host_id>/`)

```
jobs/<run_id>.json               # dispatch envelope (RunSpec — same shape as the tunnel Run frame)
jobs/<run_id>.claim              # atomic claim marker (PutMode::Create); body = claiming worker id + ts
control/<run_id>/<seq>.json      # cancel | approve{mode} | reject{reason} envelopes
runs/<run_id>/events.<seq>.jsonl # appended event lines (seq monotonic per file kind)
runs/<run_id>/step_results.<seq>.jsonl
runs/<run_id>/unit_checkpoints.<seq>.jsonl
runs/<run_id>/run.json           # latest run record (overwritten)
runs/<run_id>/finished           # terminal marker; body = final status
```

Envelopes are JSON. The result-object `<seq>` makes appends idempotent and lets
both the node (writer) and the CP poller (reader) track progress without
coordinating.

### 3. Host model + enrollment (`crates/rupu-workspace/src/host_store.rs`)

Add `HostTransport::Bucket { url: String, prefix: Option<String> }`
(serde `kind = "bucket"`). No `token_hash` / no secrets. Add
`add_bucket_host(store, name, url, prefix)` mirroring `add_ssh_host`. CLI:
`rupu host add --bucket <url> [--prefix <p>] <name>` (mutually exclusive with
`--url` / `--ssh`).

### 4. `BucketHostConnector` (`crates/rupu-cp/src/host/bucket/connector.rs`) impl `HostConnector`

Holds `{ host_id, bucket: Arc<dyn Bucket>, mirror: Arc<NodeMirror>, run_store: Arc<RunStore>, pricing }`.

- `launch_run`/`launch_agent`: mint `run_<ULID>` → `mirror.create_run(run_id, host_id, spec)` → `bucket.put_job(run_id, RunSpec envelope)` → return id.
- `cancel_run`/`approve_run`/`reject_run`: `bucket.put_control(run_id, next_seq, envelope)` (queued; applied on the node's next poll). These return `Ok` once the control envelope is written — the operator sees it take effect after the poll latency.
- Observation (`list_runs`/`get_run`/`stream_run_events`/`get_transcript`): the **shared mirror-backed helpers** (2c) filtered by `host_id`. `proxy_get_json` → `Invalid`. sessions → `Invalid("sessions not supported over bucket (slice 2b)")`.
- `info().reachable` ← `bucket.probe()` ok.

### 5. CP-side bucket poller (`crates/rupu-cli/src/cmd/cp.rs` background task)

A sibling of the tunnel read-pump: in `rupu cp serve`, periodically, for each
`Bucket` host, `bucket.list_results(run_id)` for the host's in-flight runs (and
discover new ones from the mirror's Running set), feed new `<seq>` lines to
`NodeMirror::append`, overwrite via `RunJson`, and on `get_finished` call
`NodeMirror::finish`. Track the highest consumed `<seq>` per run (in memory) so
each object is mirrored once. Bounded interval; reuses the `HostStore` already
threaded into the resume worker to discover Bucket hosts.

### 6. `rupu node pull` agent (`crates/rupu-cli/src/cmd/node.rs`)

`rupu node pull --bucket <url> [--prefix p] [--host-id <id>] [--once] [--interval <secs>]`:
loop (default) — each tick: `list_jobs` → for each unclaimed, `claim_job`
(atomic; skip if `false`); on a won claim, `get_job`, spawn the local run
(`rupu workflow run --run-id …` / `rupu run …`, the same argv builders as ssh/tunnel),
tail its artifact files and `put_result` new lines + `run.json`, drain
`list_control` envelopes (cancel → kill child; approve/reject → shell
`rupu workflow approve|reject`), and on terminal status `put_finished`. `--once`
drains current jobs then exits (cron); otherwise sleep `--interval` (default
e.g. 15s) and repeat. The node authenticates to the bucket via the env cred
chain (no rupu-held secret).

### 7. Resume-worker filter (`crates/rupu-cli/src/cmd/cp.rs`)

Extend the `remote_workers` set (currently Tunnel `node_id` + Ssh `h.id`) to also
include `Bucket` hosts (`Bucket { .. } => Some(h.id)`) — bucket runs resume via a
queued control envelope, never the central local worker.

## Errors & security

- No secrets in rupu: the `Bucket` host record holds only `url` + `prefix`;
  credentials come from the cloud cred chain / env. `object_store` handles
  signing.
- Atomic claim (`PutMode::Create`) guarantees at-most-one node runs a job even
  with multiple pull nodes on one bucket; a lost claim is a clean skip.
- The mirror write path is the hardened 2.5 one (`worker_id`-scoped,
  run_id-validated, `resume_*` nulled on import); the CP poller writes through
  `NodeMirror` with `host_id` as `worker_id`.
- Envelope `run_id`s the node/CP act on are CP-minted ULIDs (charset-validated by
  `validate_run_id` before any RunStore op), so bucket keys can't traverse the
  central run store.
- `#![deny(clippy::all)]`; no `unsafe`; library errors `thiserror`, CLI `anyhow`.

## Testing

- **Host model:** `HostTransport::Bucket` serde round-trip; `add_bucket_host`.
- **Bucket port (CI via `object_store::memory::InMemory` / `LocalFileSystem`):**
  put/list/get jobs; `claim_job` returns true once then false (atomic claim);
  control put/list ordering; result put/list with `<seq>`; finished marker;
  probe. These exercise the REAL `ObjectStoreBucket` code path with no cloud.
- **`BucketHostConnector` (in-memory bucket):** `launch_run` mints id + creates
  mirror run + writes a job envelope; cancel/approve/reject write control
  envelopes; observation reads the mirror; offline bucket (probe fails) →
  `info().reachable == false`.
- **CP poller:** seed result objects in an in-memory bucket → the poller mirrors
  them into the RunStore (list/detail/events) and `finish`es on the marker;
  idempotent (re-poll doesn't double-append).
- **Pull agent claim:** two `claim_job` calls on the same run against one
  in-memory bucket → exactly one wins (atomic).
- **Resume-worker filter:** a Bucket-attributed awaiting run is not picked up by
  the resume worker.
- **e2e (in-memory bucket):** CP `launch_run` writes a job → a simulated pull
  step claims + writes result/finished envelopes → the CP poller mirrors → the
  central run shows completed; a control envelope (cancel) is delivered and seen.

## Open questions

- **Bucket retention / GC:** consumed jobs/claims/results accumulate. 2b does a
  basic best-effort cleanup of a run's bucket keys after the CP poller sees the
  `finished` marker AND has mirrored everything; a full retention policy is
  future work.
- **Multiple pull nodes per host_id vs per bucket:** 2b scopes a bucket to one
  `host_id` prefix; running several nodes against the same prefix is supported
  (atomic claim load-balances), but treating distinct nodes as distinct hosts is
  future work.
