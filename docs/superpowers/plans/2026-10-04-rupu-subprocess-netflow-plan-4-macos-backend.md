# Subprocess netflow capture — Plan 4: the macOS backend

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans. Checkbox (`- [ ]`) steps.

**Goal:** A macOS capture backend in `rupu-netwatch` that observes the TCP/UDP connections a bash call's processes make — via the kernel's network-statistics control (`com.apple.network.statistics`, the feed behind `nettop`) — and feeds the pure `Tracker` (Plan 2), attributing by process ancestry. `net_capture::shared` returns it on macOS when it can start, else the unsupported backend. No bash wiring (Plan 5).

**Architecture:** Mirror Plan 3's shape. PURE cross-platform modules decode the ntstat message/descriptor bytes → an observation (unit-tested against a real descriptor fixture captured on this Mac). A `#[cfg(target_os="macos")]` IO shell opens the `PF_SYSTEM`/`SYSPROTO_CONTROL` control socket (via `nix`, no `unsafe`), subscribes to all TCP+UDP sources, requests each new source's description, and runs a watcher thread that resolves each socket's owning process to the bash call that spawned it (walking the parent-pid chain with `libproc` up to a registered shell pid = the tracker's `OwnerId`) and feeds the `Tracker`. **This Mac is the build+test platform** — the implementer compiles and runs everything here, including the `#[ignore]` live integration test (a real `curl` whose flow must be attributed to the call).

**Tech stack:** Rust 2021; new macOS-only deps `nix` (socket + `SysControlAddr::from_name`, which wraps `CTLIOCGINFO` safely) and `libproc` (parent pid). No `unsafe` (the whole crate forbids it); no raw FFI. Struct layouts ported from xnu `bsd/net/ntstat.h` rev 9, pinned by a size/offset test.

**Spec:** `docs/superpowers/specs/2026-10-04-rupu-subprocess-netflow-capture-design.md` §10 (and §10.5 the detached-child gap; §7.5 field mapping).

## Global Constraints
- **No `unsafe`.** If a needed call has no safe `nix`/`libproc`/std wrapper, STOP and report — do not write FFI (spec §18 V2). `nix` + `libproc` are safe wrapper crates; add them to root `[workspace.dependencies]` and reference `.workspace = true` in the crate (never pin versions in the crate manifest). They are macOS-only — declare them under `[target.'cfg(target_os = "macos")'.dependencies]`.
- `#![deny(clippy::all)]`.
- **Pure vs cfg(macos) split:** the ntstat byte parser is plain cross-platform logic (compiles + tests on any OS). Only the control socket, libproc ancestry, and the watcher thread are `#[cfg(target_os="macos")]`.
- **Capture never breaks the bash call; watcher never blocks the request path; visible loss is surfaced** (spec §13) — same invariants as the Linux backend: panic-safe watcher (`catch_unwind`), mutex-poison recovery, `ENOBUFS` noted as a `capture_state` line, a dead watcher → inert `begin`.
- **Attribution is by `OwnerId` = the registered call's shell pid.** The watcher resolves a socket's owning pid (from the descriptor) up its parent chain to a registered shell pid. The known detached-child gap (§10.5): a double-forked/`setsid` child whose chain reaches `launchd` is reported as an `Origin::Subprocess` flow with `tool_call_id: None`, never guessed into a call.
- **The ntstat API is private/undocumented** (the `nettop` feed). The size/offset test (Task 1) guards the struct layout; a `__NSTAT_REVISION__` mismatch logs a one-time warning but does not disable capture.

## File structure

| File | Responsibility | cfg |
|---|---|---|
| `crates/rupu-netwatch/src/macos/mod.rs` | module wiring; `MacosCapture`; live `#[ignore]` test | macos for `MacosCapture`; decl unconditional |
| `crates/rupu-netwatch/src/macos/parse.rs` | PURE: ntstat `nstat_msg_hdr` + tcp/udp descriptor bytes → `NstatObservation` | none |
| `crates/rupu-netwatch/src/macos/ntstat.rs` | IO: the control socket (open/connect/subscribe/request-desc/recv) + message protocol | macos |
| `crates/rupu-netwatch/src/macos/proctree.rs` | IO: `libproc` parent-pid walk → owning registered shell pid | macos |
| `crates/rupu-netwatch/src/macos/watch.rs` | IO: watcher thread — ntstat events → ancestry → `Tracker` → deliver | macos |
| `crates/rupu-netwatch/src/lib.rs` | `pub mod macos;` | — |
| root `Cargo.toml` | add `nix`, `libproc` to `[workspace.dependencies]` | — |
| `crates/rupu-netwatch/Cargo.toml` | macOS-target deps: `nix` (features socket+ioctl), `libproc` | — |

---

### Task 1: deps + PURE ntstat descriptor parser (real fixture)

**Files:** root `Cargo.toml` (+nix,+libproc workspace deps); `crates/rupu-netwatch/Cargo.toml` (macos target deps); create `macos/mod.rs`, `macos/parse.rs`; `lib.rs`. Fixture: `crates/rupu-netwatch/tests/fixtures/ntstat_tcp_descriptor.hex`.

**Interfaces — produces (PURE, no cfg):**
```rust
pub struct NstatObservation {
    pub srcref: u64,          // the source ref (tracker SocketId)
    pub transport: Transport,
    pub pid: u32,
    pub pname: String,        // from the descriptor (<=64 bytes, NUL-trimmed)
    pub local: Option<SocketAddr>,
    pub remote: Option<SocketAddr>,
    pub state: u32,           // TCP state (ignored for UDP)
    pub established: bool,
    pub bytes_in: Option<u64>,
    pub bytes_out: Option<u64>,
}
pub enum NstatMsg {         // the message kinds the watcher acts on
    SrcAdded { srcref: u64, provider: u32 },
    SrcDesc(NstatObservation),
    SrcRemoved { srcref: u64 },
    Other,
}
/// Parse one ntstat message (hdr + body) from `bytes`. Returns the kind.
pub fn parse_message(bytes: &[u8]) -> NstatMsg;
/// Parse a TCP or UDP descriptor body into an observation.
pub fn parse_tcp_descriptor(desc: &[u8], srcref: u64) -> Option<NstatObservation>;
pub fn parse_udp_descriptor(desc: &[u8], srcref: u64) -> Option<NstatObservation>;
```

- [ ] **Step 1: deps.** Root `[workspace.dependencies]`: `nix = { version = "0.31", default-features = false }` and `libproc = "0.14"` (pick the current pinned version; confirm it builds). In `rupu-netwatch/Cargo.toml` add `[target.'cfg(target_os = "macos")'.dependencies]` with `nix = { workspace = true, features = ["socket", "ioctl"] }` and `libproc = { workspace = true }`. Add `pub mod macos;` to `lib.rs`; `macos/mod.rs` declares `pub mod parse;` and (cfg macos) `pub mod ntstat; pub mod proctree; pub mod watch;` as `//!` stubs.
- [ ] **Step 2: capture a real fixture (on this Mac).** Write a throwaway helper (a `#[ignore]` test or a scratch bin) that opens the ntstat control socket, subscribes TCP, and on the first `SRC_DESC` for a live socket writes the raw message bytes (hdr+descriptor) as hex to `tests/fixtures/ntstat_tcp_descriptor.hex`. Reference the proven C approach in the spike (`/private/tmp/.../scratchpad/ntstat/nwatch.c`) for the message/struct layout. Commit the fixture. (If the socket can't be opened in the sandbox, capture by running the helper directly; the fixture is real kernel bytes.)
- [ ] **Step 3: failing tests** in `parse.rs` decoding the fixture: `parses_tcp_descriptor_fixture` → assert the decoded `pid`, `pname` (e.g. the capturing process), `local`/`remote` (decode to pin exact values), `srcref`, `state`, and that `bytes_in`/`bytes_out` are `Some`. Plus `parse_message_classifies_src_desc`, and `short_buffer_is_other_or_none`.
- [ ] **Step 4: implement** per xnu `ntstat.h` rev 9: `nstat_msg_hdr { context u64, type u32, length u16, flags u16 }`; types `SRC_ADDED=10001, SRC_REMOVED=10002, SRC_DESC=10003, SRC_UPDATE=10006`; providers `TCP_KERNEL=2, UDP_KERNEL=4`. `nstat_tcp_descriptor`/`nstat_udp_descriptor` field offsets as in the spike's `nwatch.c` (upid/eupid/start/ts/… then `pid`, `local`/`remote` as `sockaddr_in`/`sockaddr_in6` unions, `pname[64]`). Length-guard every read (short → None). Map `0.0.0.0:0`/`[::]:0` remote → None.
- [ ] **Step 5:** run green on macOS; clippy. **Also `cargo build -p rupu-netwatch` to confirm nix/libproc resolve.**
- [ ] **Step 6: commit.** `netwatch(macos): deps + pure ntstat descriptor parser over a real fixture`

---

### Task 2 (cfg macos): the ntstat control socket + size/offset guard

**Files:** `macos/ntstat.rs`; a struct-layout test.

**Interfaces — produces (`#[cfg(target_os="macos")]`):**
```rust
pub struct NstatSocket { /* owns the control-socket OwnedFd */ }
impl NstatSocket {
    pub fn open() -> io::Result<Self>;             // socket(PF_SYSTEM, DGRAM, SYSPROTO_CONTROL) + connect via SysControlAddr::from_name("com.apple.network.statistics")
    pub fn subscribe_all(&self) -> io::Result<()>; // ADD_ALL_SRCS for TCP_KERNEL + UDP_KERNEL (filters: accept all ifaces; USE_UPDATE_FOR_ADD)
    pub fn request_description(&self, srcref: u64) -> io::Result<()>; // GET_SRC_DESC
    pub fn recv_into(&self, buf: &mut [u8]) -> io::Result<usize>;
    pub fn set_recv_timeout(&self, d: Option<Duration>) -> io::Result<()>;
}
```

- [ ] **Step 1: implement** with `nix::sys::socket` — `socket(AddressFamily::System, SockType::Datagram, SockProtocol?/raw SYSPROTO_CONTROL)`, `SysControlAddr::from_name(fd, "com.apple.network.statistics", 0)`, `connect`. Build the request messages (ADD_ALL_SRCS etc.) as byte buffers matching `ntstat.h` (reuse the constants from Task 1). `set_recv_timeout` via `nix` `setsockopt(.., ReceiveTimeout, ..)`. Confirm exact nix paths against the installed source.
- [ ] **Step 2: size/offset test** (`#[cfg(target_os="macos")]`, NOT ignored — pure constants): assert `size_of`/`offset_of` (via `memoffset` or manual const math) for the hdr + tcp/udp descriptors match the rev-9 values the parser assumes, so a wrong offset fails at build/test time, not at runtime.
- [ ] **Step 3: `#[ignore]` smoke test** (macos): `open()`, `subscribe_all()`, recv a few messages, assert at least one parses as `SrcAdded`/`SrcDesc` via `parse_message`. Runnable on this Mac.
- [ ] **Step 4:** implementer runs on this Mac: `cargo test -p rupu-netwatch` (incl `-- --ignored macos::ntstat`), clippy. Commit: `netwatch(macos): network-statistics control socket + struct-layout guard`

---
### Task 3 (cfg macos): ancestry + `MacosCapture` + watcher

**Files:** `macos/proctree.rs`, `macos/watch.rs`, `MacosCapture` in `macos/mod.rs`. All cfg macos.

**Interfaces — produces:**
```rust
// proctree.rs
/// Walk `pid`'s parent chain (via libproc `pidinfo::<BSDInfo>`), up to `max`
/// hops, returning the first ancestor (inclusive of pid) that is in `is_owner`.
pub fn owning_ancestor(pid: u32, max: u32, is_owner: impl Fn(u32) -> bool) -> Option<u32>;

// mod.rs
pub struct MacosCapture { /* Arc<Mutex<Tracker>>, registered shell pids set, thread, runtime */ }
impl MacosCapture { pub fn start(linger: chrono::Duration) -> Result<MacosCapture, String>; }
// impl rupu_netflow::SubprocessCapture for MacosCapture
```

- [ ] **Step 1: implement proctree** with `libproc::proc_pid::pidinfo::<libproc::bsd_info::BSDInfo>(pid as i32, 0)` → `pbi_ppid`; stop at pid<=1 or `max` hops. Unit-testable indirectly; a `#[ignore]` test asserts `owning_ancestor(std::process::id(), 8, |p| p == parent)` finds this process's own parent.
- [ ] **Step 2: implement the watcher** (one thread, owns a current-thread tokio runtime for sink delivery like the Linux backend): `MacosCapture::start` opens an `NstatSocket`, `subscribe_all`, `set_recv_timeout(poll)`, spawns the thread. Loop (panic-wrapped, `catch_unwind`): `recv_into`; `parse_message`:
  - `SrcAdded { srcref }` → `request_description(srcref)` (resolve ancestry lazily at `SrcDesc`, while the process is alive).
  - `SrcDesc(obs)` → resolve `owning_ancestor(obs.pid, 32, |p| registered_shells.contains(p))`; if `Some(shell)`, the `OwnerId` is `shell` → build a `SocketSnapshot` (process = Some{pid,pname}) → `tracker.observe(srcref, shell as u64, snapshot, now)`. If `None`, ignore (unattributed; tracker drops unknown owners).
  - `SrcRemoved { srcref }` → `tracker.close(srcref, last_snapshot_or_minimal, now)`.
  - periodically `tracker.tick(now)`.
  Deliver every `Emission` to its sink (collect under the tracker lock; deliver via `rt.block_on`, each wrapped in `catch_unwind`). `ENOBUFS` → one `capture_state` loss note per live call, continue.
- [ ] **Step 3: `SubprocessCapture for MacosCapture`:** `begin(call)` → record the call's shell pid (the pid passed to `spawned()`), `tracker.register_call(seq, CallInfo{ owner: shell_pid, attribution }, now)`, return a `CaptureCall` whose `shell_prefix()` is `None` (macOS needs no prefix — attribution is by ancestry) and whose `spawned(pid)` records `pid` as the call's owner shell pid (update both the tracker's owner mapping and the registered-shells set) and `finished()` → `finish_call`. **Important:** because `OwnerId` is the shell pid and `begin` happens before the pid is known, register the call on `spawned(pid)` (or register in `begin` with a placeholder and re-key on `spawned`). Simplest: defer `register_call` to `spawned(pid)`, where the real pid is known; `begin` just stores the attribution. `run_finished(run_id)` → `finish_run`.
- [ ] **Step 4: live `#[ignore]` integration test** (macos, runnable on this Mac): `MacosCapture::start(3s)`; `begin` a call with a `MemorySink`; spawn `/bin/sh -c "curl -s -o /dev/null --max-time 5 http://example.com"`, pass its pid to `spawned()`; wait ~1s; `finished()`; assert the sink received a Socket-fidelity flow, origin `Subprocess`, `ctx.tool_call_id` set, remote port 80. (Resilient asserts: exact IP varies.)
- [ ] **Step 5:** implementer runs on this Mac: `cargo test -p rupu-netwatch -- --ignored macos::` passes (incl the live test); clippy. Commit: `netwatch(macos): libproc ancestry + MacosCapture watcher feeding the tracker`

---

### Task 4: wire `net_capture::shared` to the macOS backend

**Files:** `crates/rupu-runtime/src/net_capture.rs`.

- [ ] **Step 1:** add a `#[cfg(target_os = "macos")]` enabled arm to `choose`: try `MacosCapture::start(Duration::milliseconds(linger_ms clamped))` (use `try_milliseconds` + clamp — also fixing the Plan-3 deferred panic-on-absurd-config for the shared helper); `Ok` → it, `Err(reason)` → `UnsupportedCapture::new(reason)`. Linux arm unchanged; other targets unchanged; disabled/env-off → Noop.
- [ ] **Step 2:** a `#[cfg(target_os="macos")]` `#[ignore]` test `enabled_on_macos_returns_a_working_backend`: `choose(enabled,false)` begins a call, `spawned(std::process::id())`, asserts it's a real backend (not a silent Noop) — e.g. a later observe path works, or simply that `begin`/`spawned`/`finished` run without error and the backend is `MacosCapture` (assert via a trait probe or that no Unavailable note was written when start succeeded).
- [ ] **Step 3:** macOS `cargo test -p rupu-runtime` green (incl `-- --ignored`); clippy; `cargo build -p rupu-cli -p rupu-cp`. Commit: `runtime: net_capture::shared returns the macOS backend when available`

---

## Final verification
- [ ] macOS: `cargo test -p rupu-netwatch -p rupu-runtime` green; `cargo test -p rupu-netwatch -p rupu-runtime -- --ignored` green (the live tests run on THIS Mac); `cargo clippy --all-targets` clean; `cargo build -p rupu-cli -p rupu-cp`.
- [ ] **Linux still builds** (the macos code is cfg-excluded): rsync to kali6 and `cargo build -p rupu-netwatch -p rupu-runtime` (confirm no accidental non-cfg breakage). [controller]
- [ ] No premature bash wiring: `git grep net_capture::shared crates/rupu-tools crates/rupu-agent` empty.

## Notes for later plans
- Plan 5 wires bash → `net_capture::shared`; on THIS Mac the macOS backend now makes full end-to-end local testing of the bash path possible (the first real socket flows from a real agent bash call). Honor the Plan-3 carry: call `shared()` off the async runtime.
- Detached-child gap (§10.5) stands; a future descendant-pre-registration sweep could close it.
