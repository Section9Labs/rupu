# Subprocess netflow capture — Plan 3: the Linux backend

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans. Steps use checkbox (`- [ ]`) syntax.

**Goal:** A Linux capture backend in `rupu-netwatch` that observes the TCP/UDP connections a bash call's processes make and feeds the existing pure `Tracker`, attributing by cgroup. No bash wiring yet (Plan 5); `net_capture::shared` returns it on Linux when a runtime check passes, else the unsupported backend.

**Architecture:** Split into cross-platform PURE modules (byte parsing, request building, cgroup-path decision — unit-tested everywhere against real `sock_diag` fixtures) and a thin `#[cfg(target_os="linux")]` IO shell (a `NETLINK_SOCK_DIAG` socket via `rustix`, cgroup directory management via `std::fs`, and a watcher thread that polls the socket table + reads the close multicast and drives the `Tracker`). Attribution: each bash call gets its own cgroup under a delegated root; the cgroup id (the cgroup directory's inode) is the tracker's `OwnerId`; a live dump maps socket cookie → cgroup id, and the close multicast finalizes by cookie.

**Tech Stack:** Rust 2021, `rustix` (add the `net` feature) for the netlink socket, `std::fs` for cgroupfs, `std::process::Command` for the one-shot `busctl` scope creation, the `Tracker`/`types` from Plan 2. No `unsafe` (verified: `rustix::net` exposes `AddressFamily::NETLINK` + `netlink::SocketAddrNetlink` + raw `Protocol`, and all message bytes are built/parsed as `&[u8]`). No heavy netlink crate.

**Spec:** `docs/superpowers/specs/2026-10-04-rupu-subprocess-netflow-capture-design.md` §9 (and §9.4 the cookie→cgroup join; §7.5 field mapping).

**Build/verify environment:** this Mac cannot compile `#[cfg(target_os="linux")]` code (Homebrew rustc, no rustup/Linux std). PURE modules build + test on macOS. The cfg(linux) IO and the `#[ignore]` live test are built and run on the LAN Kali VM **kali6** (`kali@192.168.64.6`, key `~/.ssh/id_ed25519_kali`; kernel 6.19 arm64, cgroup v2, systemd user session, cargo 1.98). The controller rsyncs the repo to kali6 and runs `cargo test -p rupu-netwatch` (and `-- --ignored` for the live test) there after each cfg(linux) task, then cleans the remote `target/`. Batch SSH into one invocation (LAN, but respect the connection-burst rule).

## Global Constraints
- **No `unsafe`.** If an implementer finds a required call has no safe `rustix`/std wrapper, STOP and report — do not add an FFI/unsafe shim (spec §18 V2).
- `#![deny(clippy::all)]`; workspace dependency versions only (add `net` to the existing root `rustix` feature list; do not bump its version).
- **Pure vs IO split is mandatory:** parsing, request building, and the cgroup-mode *decision* are plain cross-platform functions with no syscalls, so they compile and are unit-tested on macOS. Only actual syscalls/fs mutation/threads are `#[cfg(target_os="linux")]`.
- **Capture never affects the bash call** and **the watcher never blocks the request path** (spec §13): every backend error becomes an `Unavailable`/note, never a failure or slowdown.
- Real fixtures already on the branch: `crates/rupu-netwatch/tests/fixtures/sock_diag_tcp_{listener,established}.hex` (real kernel bytes). The parser tests decode these — do not replace them with synthesized bytes.

---
## File structure

| File | Responsibility | cfg |
|---|---|---|
| `crates/rupu-netwatch/src/linux/mod.rs` | module wiring; `LinuxCapture` (the backend); live `#[ignore]` test | linux for `LinuxCapture`; module decl unconditional |
| `crates/rupu-netwatch/src/linux/parse.rs` | PURE: `inet_diag` msg + attrs → `InetDiagObservation`; close-msg → `(SocketId, final snapshot)` | none (cross-platform) |
| `crates/rupu-netwatch/src/linux/req.rs` | PURE: build the dump request bytes; the multicast group mask; netlink msg framing | none |
| `crates/rupu-netwatch/src/linux/cgroup.rs` | PURE decision: parse `/proc/self/cgroup`, choose `Mode::{Scope,Direct,Unavailable}` from inputs; IO: create/enter/remove call cgroups | decision PURE; fs ops linux |
| `crates/rupu-netwatch/src/linux/netlink.rs` | IO: open/bind/send/recv a `NETLINK_SOCK_DIAG` socket (rustix) | linux |
| `crates/rupu-netwatch/src/linux/watch.rs` | IO: watcher thread — poll + close-multicast → `Tracker` → deliver emissions | linux |
| `crates/rupu-netwatch/src/lib.rs` | `pub mod linux;` | — |
| root `Cargo.toml` | `rustix` gains `"net"` | — |

The pure modules (`parse`, `req`, `cgroup`-decision) hold the logic; the IO modules are thin. `LinuxCapture` owns a `std::sync::Mutex<Tracker>` and a background thread; `begin` creates a call cgroup and returns a `CaptureCall` whose `shell_prefix` enters it. The `OwnerId` is the call cgroup's inode.

---

### Task 1: `rustix net` + PURE netlink/inet_diag parser (fixtures)

**Files:** root `Cargo.toml`; create `crates/rupu-netwatch/src/linux/mod.rs`, `linux/parse.rs`; modify `lib.rs`. Tests in `parse.rs`.

**Interfaces — produces:**
```rust
// parse.rs — PURE, no cfg
pub struct InetDiagObservation {
    pub socket_id: u64,       // idiag_cookie (two u32 joined)
    pub transport: Transport, // caller passes which dump this came from
    pub family_v4: bool,
    pub state: u8,            // idiag_state (1=ESTABLISHED, 10=LISTEN, ...)
    pub local: Option<SocketAddr>,
    pub remote: Option<SocketAddr>,
    pub cgroup_id: Option<u64>,   // INET_DIAG_CGROUP (type 21), when present
    pub bytes_in: Option<u64>,    // tcp_info.tcpi_bytes_received via INET_DIAG_INFO (type 2)
    pub bytes_out: Option<u64>,   // tcp_info.tcpi_bytes_acked
    pub established: bool,         // state transitioned to ESTABLISHED or beyond
}
/// Parse one `inet_diag_msg` (the bytes AFTER the 16-byte nlmsghdr) for a dump
/// of `transport`. Returns None if too short / malformed.
pub fn parse_inet_diag(body: &[u8], transport: Transport) -> Option<InetDiagObservation>;
/// Iterate the NLMSGs in a recv buffer, yielding (nl_type, body slice).
pub fn nlmsgs(buf: &[u8]) -> impl Iterator<Item = (u16, &[u8])>;
```

- [ ] **Step 1: add `"net"`** to `rustix` in root `Cargo.toml` (`features = ["process", "fs", "net"]`); add `pub mod linux;` to `lib.rs`; create `linux/mod.rs` with `pub mod parse;` (and `pub mod req; pub mod cgroup;` as empty `//!` stubs for later tasks).
- [ ] **Step 2: failing tests** in `parse.rs` that load the two fixtures:
```rust
fn load(name: &str) -> Vec<u8> {
    let hex = std::fs::read_to_string(
        concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/")).unwrap(); // see note
    // read `<fixtures>/<name>` and hex-decode its single line
}
```
  Write `parses_established_fixture_with_remote_and_cookie`: load `sock_diag_tcp_established.hex`, strip the 16-byte nlmsghdr, `parse_inet_diag(body, Transport::Tcp)` → assert `state == 1`, `family_v4`, `local`/`remote` are the addresses the bytes encode (decode them in the test to pin exact values — do NOT hand-wave), `socket_id` equals the idiag_cookie, and that an `INET_DIAG_INFO` attribute yielded `bytes_in`/`bytes_out` as `Some`. And `parses_listener_fixture_without_remote`: the listener fixture → `state == 10`, `remote` is the all-zero address (treat `0.0.0.0:0` as "no remote" — map to `None`). Add a `malformed_short_buffer_is_none` test.
- [ ] **Step 3: run** `cargo test -p rupu-netwatch --lib linux::parse` → fail.
- [ ] **Step 4: implement** `parse_inet_diag` + `nlmsgs`. `inet_diag_msg` layout: `idiag_family(1) idiag_state(1) idiag_timer(1) idiag_retrans(1)`, then `idiag_sockid` = `sport(2 BE) dport(2 BE) src(16) dst(16) if(4) cookie(8 = two u32 LE: cookie[0] | cookie[1]<<32)`, then `idiag_expires(4) idiag_rqueue(4) idiag_wqueue(4) idiag_uid(4) idiag_inode(4)`, then rtattr TLVs: each `rta_len(2) rta_type(2)` + payload, 4-aligned. Type 2 = `INET_DIAG_INFO` (a `tcp_info`; read `tcpi_state` byte at offset 0, `tcpi_bytes_acked` and `tcpi_bytes_received` — their offsets vary by kernel, so guard with length checks and read via documented offsets for the modern `tcp_info`; if the struct is shorter than the needed offset, leave bytes `None`). Type 21 = `INET_DIAG_CGROUP` (a u64). `established = state == 1 || state == TCP_ESTABLISHED-or-later per the idiag_state`. Map a `0.0.0.0`/`::`+port-0 remote to `None`.
- [ ] **Step 5: run tests green; clippy.** `cargo test -p rupu-netwatch` and `cargo clippy -p rupu-netwatch --all-targets` (all on macOS — these modules are cfg-free).
- [ ] **Step 6: commit.** `netwatch(linux): pure inet_diag parser over real sock_diag fixtures`

---

### Task 2: PURE request builder + cgroup-mode decision

**Files:** `linux/req.rs`, `linux/cgroup.rs` (decision part only). Tests inline.

**Interfaces — produces:**
```rust
// req.rs — PURE
/// Bytes of a SOCK_DIAG_BY_FAMILY dump request for (family, proto), requesting
/// INET_DIAG_INFO + cgroup id, all states. seq is echoed back by the kernel.
pub fn dump_request(family: u8, proto: u8, seq: u32) -> Vec<u8>;
/// The multicast group bitmask to bind for TCP+UDP v4+v6 destroy events.
pub const DESTROY_GROUPS: u32; // (1<<0)|(1<<1)|(1<<2)|(1<<3)

// cgroup.rs — PURE decision
pub enum Mode { Scope, Direct, Unavailable(String) }
pub struct CgroupEnv<'a> { pub self_cgroup: &'a str, pub cgroup2_mounted: bool,
    pub writable: bool, pub delegated: bool, pub systemd_available: bool, pub uid: u32 }
/// Decide how to obtain a delegated capture root (spec §9.1), first match wins.
pub fn choose_mode(env: &CgroupEnv) -> Mode;
```

- [ ] **Step 1: failing tests.** `req`: `dump_request(AF_INET=2, IPPROTO_TCP=6, 1)` begins with a 16-byte nlmsghdr whose `nlmsg_type == 20` (SOCK_DIAG_BY_FAMILY), `nlmsg_flags == NLM_F_REQUEST|NLM_F_DUMP (0x301)`, `nlmsg_seq == 1`, and whose body sets `sdiag_family==2`, `sdiag_protocol==6`, the ext byte requests INET_DIAG_INFO, and `idiag_states==0xffffffff`; total len matches `nlmsg_len`. `DESTROY_GROUPS == 0b1111`. `cgroup`: a `*.service` leaf that is not writable with systemd available → `Mode::Scope`; a leaf with `delegated==true` and `writable==true` → `Mode::Direct`; neither (no systemd, not writable) → `Mode::Unavailable(reason)`; `cgroup2_mounted==false` → `Unavailable("cgroup v2 not mounted")`.
- [ ] **Step 2-5:** run→fail, implement, run→green, clippy (all macOS). Match the §9.1 precedence exactly: cgroup2 required first; Scope when the leaf is not a bare `*.service` and systemd is reachable; Direct when the current cgroup is writable and delegated (or a cgroup-namespace root, i.e. `self_cgroup == "/"`); else Unavailable naming the cgroup and why.
- [ ] **Step 6: commit.** `netwatch(linux): pure sock_diag request builder + cgroup-mode decision`

---
### Task 3 (cfg linux, verified on kali6): netlink socket + cgroup fs ops

**Files:** `linux/netlink.rs` (new, cfg linux), `linux/cgroup.rs` (add the fs ops, cfg linux).

**Interfaces — produces (all `#[cfg(target_os = "linux")]`):**
```rust
// netlink.rs
pub struct DiagSocket { /* owns a rustix OwnedFd */ }
impl DiagSocket {
    pub fn open() -> std::io::Result<Self>;               // socket(NETLINK, RAW, SOCK_DIAG)
    pub fn bind_destroy_groups(&self) -> std::io::Result<()>; // bind nl_groups = DESTROY_GROUPS
    pub fn send(&self, bytes: &[u8]) -> std::io::Result<()>;
    pub fn recv_into(&self, buf: &mut [u8]) -> std::io::Result<usize>;
    pub fn set_rcvbuf(&self, bytes: usize);                 // best-effort SO_RCVBUF bump
}
// cgroup.rs (fs ops)
pub struct CaptureRoot { /* path to the delegated root + "supervisor" leaf */ }
pub fn setup_root() -> Result<CaptureRoot, String>;         // §9.1 steps 1-4 via choose_mode + busctl
impl CaptureRoot {
    pub fn create_call(&self, seq: u64) -> std::io::Result<CallCgroup>; // mkdir call-<seq>
}
pub struct CallCgroup { pub id: u64 /* dir inode */, pub procs_path: PathBuf }
impl CallCgroup { pub fn shell_prefix(&self) -> String; pub fn remove_if_empty(&self); }
```

- [ ] **Step 1: write the IO.** `DiagSocket` uses `rustix::net::{socket_with/socket, bind, send, recv}` with `AddressFamily::NETLINK` and the sock_diag protocol (protocol number 4; use `rustix::net::netlink` constants if present, else `Protocol::from_raw`). `bind_destroy_groups` binds a `SocketAddrNetlink` with `groups = DESTROY_GROUPS`. `setup_root` reads `/proc/self/cgroup` + checks cgroup2 mount + writability + `Delegate` (via `systemctl [--user] show -p Delegate --value <unit>`) + systemd reachability, calls `choose_mode`, and for `Scope` runs `busctl --user call ... StartTransientUnit "rupu-netwatch-<pid>-<rand>.scope" ... Delegate=true` then polls `/proc/self/cgroup` until it shows the scope; creates `R/supervisor` and moves self in (write `0` to its `cgroup.procs`); never writes `cgroup.subtree_control`. `CallCgroup::id` is `std::fs::metadata(dir).ino()`. `shell_prefix` returns `{ printf '%d\n' "$$" > '<procs_path>'; } 2>/dev/null`.
- [ ] **Step 2: smoke test** behind `#[cfg(target_os="linux")]` `#[ignore]`: open a `DiagSocket`, send `dump_request(2,6,1)`, recv, and assert `nlmsgs()` yields at least one `parse_inet_diag`-parseable TCP socket. And a `setup_root_then_create_and_remove_a_call_cgroup` ignored test.
- [ ] **Step 3: LINUX VERIFY (controller, on kali6).** rsync the repo to kali6 and run `cargo test -p rupu-netwatch` and `cargo test -p rupu-netwatch -- --ignored linux::`; both green. This both compile-checks the cfg(linux) code and runs the smoke tests against a real kernel. Clean the remote `target/` after. (The implementer, on macOS, cannot compile this; it writes the code and the controller runs the kali6 gate.)
- [ ] **Step 4: commit.** `netwatch(linux): NETLINK_SOCK_DIAG socket + cgroup capture-root fs ops`

---

### Task 4 (cfg linux, verified on kali6): the watcher + `LinuxCapture`

**Files:** `linux/watch.rs` (new), `linux/mod.rs` (`LinuxCapture`). All cfg linux.

**Interfaces — produces:**
```rust
#[cfg(target_os = "linux")]
pub struct LinuxCapture { /* Arc<Mutex<Tracker>>, CaptureRoot, thread handle, seq counter,
                             a map CallId->CallCgroup, a tokio Handle or own runtime for sink IO */ }
#[cfg(target_os = "linux")]
impl LinuxCapture {
    pub fn start(linger: chrono::Duration, poll: std::time::Duration) -> Result<Self, String>;
}
// impl rupu_netflow::SubprocessCapture for LinuxCapture
```

- [ ] **Step 1: implement.** `start` calls `setup_root()` (bubbling a reason on failure so `net_capture::shared` can fall back), opens two `DiagSocket`s (one bound to the destroy multicast, one for dumps), and spawns ONE watcher thread. The thread loops: every `poll`, send a dump on the dump socket and feed each parsed live socket to `tracker.observe(cookie, cgroup_id, snapshot, now)` (skip sockets whose cgroup_id isn't a known call — the tracker already ignores unknown owners); drain the destroy socket and feed `tracker.close(cookie, snapshot, now)`; call `tracker.tick(now)`. Every emission returned is delivered to its sink (reuse the unsupported backend's sync→async pattern: spawn on a `Handle` if present, else a transient `enable_all` current-thread runtime that drains the batch). `begin(call)`: lock tracker, `seq += 1`, `root.create_call(seq)` → cgroup id; `tracker.register_call(seq, CallInfo{owner: cgroup_id, attribution}, now)`; return a `CaptureCall` holding the `shell_prefix` and, on `finished()`, `tracker.finish_call(seq, now)` + schedule the cgroup `remove_if_empty` after linger. `run_finished(run_id)`: `tracker.finish_run(run_id, now)` and deliver emissions. A dropped kernel event (recv `ENOBUFS` / seq gap) increments a counter and writes one `capture_state` note (visible loss, spec §13).
- [ ] **Step 2: live `#[ignore]` integration test** (cfg linux): start a `LinuxCapture`, `begin` a call with a `MemorySink` + its real cgroup `shell_prefix`, run a child `/bin/sh -c "<prefix>; curl -s -o /dev/null http://example.com; curl -s -o /dev/null --connect-timeout 1 http://10.255.255.1:9 || true"`, wait, `finished()`, drive a `tick` past linger, then assert the MemorySink received: a Socket-fidelity Flow to example.com's resolved IP:443-or-80 attributed to the call (origin Subprocess), and a never-established TransportError Flow for the blackhole. Allow a short settle.
- [ ] **Step 3: LINUX VERIFY on kali6** (controller): rsync + `cargo test -p rupu-netwatch -- --ignored linux::`; assert the live test passes against the real kernel. This is the real proof the backend works. Clean remote target.
- [ ] **Step 4: commit.** `netwatch(linux): watcher thread + LinuxCapture feeding the tracker`

---

### Task 5: wire `net_capture::shared` to the Linux backend

**Files:** `crates/rupu-runtime/src/net_capture.rs`.

- [ ] **Step 1: failing test** (runs on macOS; asserts the non-linux path is unchanged — still `UnsupportedCapture` when enabled). On linux the real decision is covered by the kali6 live test, so gate the linux-specific assertion behind `#[cfg(target_os="linux")]`.
- [ ] **Step 2: implement.** In `choose`, under `#[cfg(target_os = "linux")]`, when enabled try `rupu_netwatch::linux::LinuxCapture::start(linger_from_cfg, poll_from_cfg)`; on `Ok` return it, on `Err(reason)` return `UnsupportedCapture::new(reason)`. Non-linux (and disabled/env-off) unchanged. Read `subprocess_linger_ms`/`subprocess_poll_ms` from cfg.
- [ ] **Step 3: run** `cargo test -p rupu-runtime` (macOS) green; clippy. **LINUX VERIFY on kali6:** `cargo build -p rupu-runtime` compiles the linux branch; `cargo test -p rupu-runtime` green there.
- [ ] **Step 4: commit.** `runtime: net_capture::shared returns the Linux backend when available`

---

## Final verification
- [ ] macOS: `cargo test -p rupu-netwatch -p rupu-runtime` + `cargo clippy --all-targets` green (pure modules + non-linux paths).
- [ ] kali6: rsync; `cargo test -p rupu-netwatch -p rupu-runtime` and `... -- --ignored linux::` all green; `cargo clippy -p rupu-netwatch` clean; then `rm -rf` the remote `target/`.
- [ ] No premature bash wiring: `git grep net_capture::shared crates/rupu-tools crates/rupu-agent` empty (Plan 5).

## Notes for later plans
- Plan 4 (macOS) adds `macos/` feeding the SAME tracker; `net_capture::shared` gets a `#[cfg(target_os="macos")]` arm.
- Plan 5 wires bash → `net_capture::shared`; on macOS the Plan 4 backend makes end-to-end local testing possible; on Linux the kali6 live test is the proof.
- Deferred-from-Plan-2 tracker record under-assertions can get extra coverage here via the live test's richer assertions.
