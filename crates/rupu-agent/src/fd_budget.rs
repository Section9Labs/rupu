//! File-descriptor admission control for starting agent runs.
//!
//! Every agent run (a linear workflow step, each fan-out / panel / parallel
//! unit, each `dispatch_agent` sub-agent) holds a handful of descriptors for
//! its whole lifetime — HTTP sockets, its transcript writer, its netflow
//! ledger. Graph `max_concurrency` and `dispatch_agents_parallel` are
//! effectively unbounded, so a wide enough fan-out can exhaust the process's
//! `RLIMIT_NOFILE` no matter how high it is raised; every later `open` then
//! fails with EMFILE (e.g. "agent `x` not found or failed to load: ... Too
//! many open files").
//!
//! Three layers, cheapest first:
//!
//! 1. [`configure`] (called once at CLI startup) raises the soft
//!    `RLIMIT_NOFILE` — to `[runtime].max_open_files` / `--max-open-files` /
//!    `RUPU_MAX_OPEN_FILES` when set, else to 10240 (capped at the hard
//!    limit). macOS hands
//!    GUI/launchd-spawned processes a soft limit of 256, which a few dozen
//!    concurrent agents exhaust. Child processes inherit the raised limit.
//! 2. When a new agent run is about to start and usage is above a
//!    high-water mark, the limit is *grown* (doubled, up to the configured
//!    value, else the hard limit / macOS `kern.maxfilesperproc`) before
//!    anything waits.
//! 3. Only once it can't grow further are new agent runs *paced*:
//!    [`load_agent_admitted`] waits until running agents release
//!    descriptors, then loads the agent, retrying if the load itself still
//!    hits fd exhaustion. The run's answer is unchanged — only its start is
//!    delayed — and every raise/wait is logged so throttling is never silent.
//!
//! The wait is bounded ([`Policy::max_wait`]): a parent agent blocked on a
//! child it dispatched is still holding its own descriptors, so a pathological
//! all-parents-waiting state must not deadlock. Past the bound the run is
//! admitted anyway (with a warning) and the load retries decide the outcome.

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use rustix::process::{getrlimit, setrlimit, Resource, Rlimit};

use crate::loader::{load_agent, AgentLoadError};
use crate::spec::{AgentSpec, AgentSpecParseError};

/// Admission tuning. [`Policy::default`] is what production uses; tests
/// shrink the durations.
#[derive(Debug, Clone)]
pub struct Policy {
    /// Admit only while `open_fds < limit * high_water_pct / 100`.
    pub high_water_pct: u64,
    /// First backoff between usage re-checks; doubles up to `max_backoff`.
    pub initial_backoff: Duration,
    pub max_backoff: Duration,
    /// Stop pacing and admit anyway after this long (deadlock escape).
    pub max_wait: Duration,
    /// Attempts for an agent load that fails with fd exhaustion.
    pub load_attempts: u32,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            high_water_pct: 80,
            initial_backoff: Duration::from_millis(25),
            max_backoff: Duration::from_secs(1),
            max_wait: Duration::from_secs(300),
            load_attempts: 8,
        }
    }
}

/// Default startup soft limit when `max_open_files` is unset (also macOS's
/// `OPEN_MAX`). Automatic growth takes it further on demand.
const DEFAULT_TARGET: u64 = 10_240;

/// Configured growth ceiling; `0` = none configured (the effective hard
/// limit decides).
static CEILING: AtomicU64 = AtomicU64::new(0);
/// Set once a growth attempt is refused by the OS, so the hot admission
/// path stops issuing doomed `setrlimit` calls.
static GROW_EXHAUSTED: AtomicBool = AtomicBool::new(false);

/// The OS per-process descriptor cap that `getrlimit` does not reveal.
///
/// macOS accepts (and reports back) ANY soft `RLIMIT_NOFILE`, but actually
/// enforces `kern.maxfilesperproc` (verified: soft = 999999999 reads back
/// as set, yet `open` fails with EMFILE at 245760). Budget math against the
/// reported soft limit would never pace, so the real cap is read once here.
/// There is no safe sysctl binding in the workspace (`unsafe` is forbidden),
/// so this shells out to `/usr/sbin/sysctl` once and caches the answer.
/// Linux enforces its soft limit as reported (raising past `nr_open` fails
/// with EPERM), so no extra cap is needed there.
fn os_per_process_cap() -> Option<u64> {
    #[cfg(target_os = "macos")]
    {
        static CAP: OnceLock<Option<u64>> = OnceLock::new();
        *CAP.get_or_init(|| {
            let out = std::process::Command::new("/usr/sbin/sysctl")
                .args(["-n", "kern.maxfilesperproc"])
                .output()
                .ok()?;
            String::from_utf8_lossy(&out.stdout).trim().parse().ok()
        })
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

fn min_opt(a: Option<u64>, b: Option<u64>) -> Option<u64> {
    match (a, b) {
        (Some(x), Some(y)) => Some(x.min(y)),
        (x, None) => x,
        (None, y) => y,
    }
}

/// `(effective soft, effective hard, raw hard)`: what the process can really
/// open now, the most it could ever be raised to, and the raw hard value to
/// pass back to `setrlimit` unchanged.
fn limits() -> (Option<u64>, Option<u64>, Option<u64>) {
    let lim = getrlimit(Resource::Nofile);
    let cap = os_per_process_cap();
    (
        min_opt(lim.current, cap),
        min_opt(lim.maximum, cap),
        lim.maximum,
    )
}

/// Startup soft-limit targets to try, first choice first. `requested` (the
/// operator's `max_open_files`) or [`DEFAULT_TARGET`], capped at the
/// effective hard limit; `DEFAULT_TARGET` again as a fallback if the first
/// is refused. Only ever raises.
pub(crate) fn startup_candidates(
    current: Option<u64>,
    eff_hard: Option<u64>,
    requested: Option<u64>,
) -> Vec<u64> {
    let Some(current) = current else {
        return Vec::new();
    };
    let cap = |v: u64| eff_hard.map_or(v, |h| v.min(h));
    let first = cap(requested.unwrap_or(DEFAULT_TARGET));
    let mut out = vec![first];
    let fallback = cap(DEFAULT_TARGET.min(first));
    if fallback != first {
        out.push(fallback);
    }
    out.retain(|&c| c > current);
    out
}

/// Next soft limit when growing under pressure: double, capped by the
/// effective hard limit and the configured ceiling. `None` = no headroom.
pub(crate) fn next_grow(current: u64, eff_hard: Option<u64>, ceiling: Option<u64>) -> Option<u64> {
    let mut target = current.saturating_mul(2);
    if let Some(h) = eff_hard {
        target = target.min(h);
    }
    if let Some(c) = ceiling {
        target = target.min(c);
    }
    (target > current).then_some(target)
}

fn try_set_soft(target: u64, raw_hard: Option<u64>) -> bool {
    setrlimit(
        Resource::Nofile,
        Rlimit {
            current: Some(target),
            maximum: raw_hard,
        },
    )
    .is_ok()
}

/// Outcome of [`configure`], for the caller to report. `before`/`after` are
/// EFFECTIVE limits (read back, OS cap applied) — never the value asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LimitReport {
    pub before: Option<u64>,
    pub after: Option<u64>,
    pub requested: Option<u64>,
}

impl LimitReport {
    /// The operator asked for more than the OS granted.
    pub fn fell_short(&self) -> bool {
        match (self.requested, self.after) {
            (Some(r), Some(a)) => a < r,
            _ => false,
        }
    }
}

/// Raise the soft open-file limit at startup and record the growth ceiling.
/// `requested` = the operator's `max_open_files` (flag / env / config);
/// `None` raises to [`DEFAULT_TARGET`] and lets fan-out pressure grow it.
/// Best-effort: never fails the process.
pub fn configure(requested: Option<u64>) -> LimitReport {
    CEILING.store(requested.unwrap_or(0), Ordering::Relaxed);
    GROW_EXHAUSTED.store(false, Ordering::Relaxed);
    let (before, eff_hard, raw_hard) = limits();
    for target in startup_candidates(before, eff_hard, requested) {
        if try_set_soft(target, raw_hard) {
            break;
        }
    }
    let report = LimitReport {
        before,
        after: limits().0,
        requested,
    };
    if report.fell_short() {
        tracing::warn!(
            requested = requested,
            granted = report.after,
            effective_hard_limit = eff_hard,
            "could not raise the open-file limit as far as max_open_files asks; \
             the hard limit (kern.maxfilesperproc on macOS) caps it and raising \
             that needs root"
        );
    }
    report
}

/// Try to grow the soft limit because usage is high. Returns whether it grew.
pub fn grow() -> bool {
    if GROW_EXHAUSTED.load(Ordering::Relaxed) {
        return false;
    }
    let (Some(current), eff_hard, raw_hard) = limits() else {
        return false;
    };
    let ceiling = match CEILING.load(Ordering::Relaxed) {
        0 => None,
        c => Some(c),
    };
    match next_grow(current, eff_hard, ceiling) {
        Some(target) if try_set_soft(target, raw_hard) => {
            tracing::info!(
                from = current,
                to = target,
                "raised the open-file limit to admit more concurrent agent runs"
            );
            true
        }
        _ => {
            GROW_EXHAUSTED.store(true, Ordering::Relaxed);
            false
        }
    }
}

/// Current `(open descriptors, effective limit)` for this process, or `None`
/// when either can't be determined (unlimited limit, no `/dev/fd`).
///
/// `/dev/fd` lists every open descriptor on both macOS (fdesc) and Linux
/// (symlink to `/proc/self/fd`). Listing it costs one descriptor for the
/// duration of the scan, excluded from the count. If the scan itself fails
/// with EMFILE the process is by definition at its limit.
pub fn fd_usage() -> Option<(u64, u64)> {
    let limit = limits().0?;
    match open_fds() {
        // The scan's own directory descriptor is listed too; exclude it.
        Ok(fds) => Some(((fds.len() as u64).saturating_sub(1), limit)),
        Err(e) if is_fd_exhaustion(&e) => Some((limit, limit)),
        Err(_) => None,
    }
}

/// Descriptor numbers listed in `/dev/fd` — every open descriptor plus the
/// one the scan itself holds while listing.
fn open_fds() -> std::io::Result<Vec<u64>> {
    Ok(std::fs::read_dir("/dev/fd")?
        .filter_map(|e| e.ok()?.file_name().to_str()?.parse().ok())
        .collect())
}

/// EMFILE (per-process) or ENFILE (system-wide) — the "too many open files"
/// family that pacing can recover from.
pub fn is_fd_exhaustion(e: &std::io::Error) -> bool {
    matches!(
        e.raw_os_error(),
        Some(code) if code == rustix::io::Errno::MFILE.raw_os_error()
            || code == rustix::io::Errno::NFILE.raw_os_error()
    )
}

impl AgentLoadError {
    /// Whether this load failed only because the process ran out of file
    /// descriptors (the agent file itself may be perfectly fine).
    pub fn is_fd_exhaustion(&self) -> bool {
        match self {
            AgentLoadError::Io { source, .. } => is_fd_exhaustion(source),
            AgentLoadError::Parse {
                source: AgentSpecParseError::Io(e),
                ..
            } => is_fd_exhaustion(e),
            _ => false,
        }
    }
}

fn under_high_water(open: u64, limit: u64, pct: u64) -> bool {
    // u128 so a huge (e.g. 2^63) limit can't overflow the multiply.
    (open as u128) * 100 < (limit as u128) * (pct as u128)
}

/// Wait until fd usage (from `probe`) is under the high-water mark, or
/// `policy.max_wait` elapses. Above the mark it first tries `grow` (raise
/// the limit) and only sleeps when that fails. Returns how long it waited.
pub async fn admit_with(
    label: &str,
    policy: &Policy,
    probe: impl Fn() -> Option<(u64, u64)>,
    mut grow: impl FnMut() -> bool,
) -> Duration {
    let start = Instant::now();
    let mut backoff = policy.initial_backoff;
    let mut warned = false;
    loop {
        let Some((open, limit)) = probe() else {
            return start.elapsed();
        };
        if under_high_water(open, limit, policy.high_water_pct) {
            if warned {
                tracing::info!(
                    agent = label,
                    waited_ms = start.elapsed().as_millis() as u64,
                    open_fds = open,
                    fd_limit = limit,
                    "fd pressure eased; starting agent run"
                );
            }
            return start.elapsed();
        }
        if grow() {
            continue;
        }
        if start.elapsed() >= policy.max_wait {
            tracing::warn!(
                agent = label,
                open_fds = open,
                fd_limit = limit,
                waited_secs = start.elapsed().as_secs(),
                "fd pressure did not ease within the pacing window; starting agent run anyway"
            );
            return start.elapsed();
        }
        if !warned {
            tracing::warn!(
                agent = label,
                open_fds = open,
                fd_limit = limit,
                high_water_pct = policy.high_water_pct,
                "near the open-file limit; pacing agent run start until running agents release descriptors"
            );
            warned = true;
        }
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(policy.max_backoff);
    }
}

/// Retry `load` while it fails with fd exhaustion, backing off between
/// attempts. Any other error (or success) returns immediately.
pub async fn retry_on_fd_exhaustion<T>(
    label: &str,
    policy: &Policy,
    mut grow: impl FnMut() -> bool,
    mut load: impl FnMut() -> Result<T, AgentLoadError>,
) -> Result<T, AgentLoadError> {
    let mut backoff = policy.initial_backoff;
    let mut attempt = 1;
    loop {
        match load() {
            Err(e) if e.is_fd_exhaustion() && attempt < policy.load_attempts => {
                tracing::warn!(
                    agent = label,
                    attempt,
                    error = %e,
                    "agent load hit the open-file limit; retrying"
                );
                if !grow() {
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(policy.max_backoff);
                }
                attempt += 1;
            }
            other => return other,
        }
    }
}

/// [`load_agent`], paced by fd pressure and resilient to transient EMFILE.
/// Use this wherever an agent run is about to start.
pub async fn load_agent_admitted(
    global: &Path,
    project: Option<&Path>,
    name: &str,
) -> Result<AgentSpec, AgentLoadError> {
    let policy = Policy::default();
    admit_with(name, &policy, fd_usage, grow).await;
    retry_on_fd_exhaustion(name, &policy, grow, || load_agent(global, project, name)).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU32;

    fn fast() -> Policy {
        Policy {
            high_water_pct: 80,
            initial_backoff: Duration::from_millis(1),
            max_backoff: Duration::from_millis(2),
            max_wait: Duration::from_millis(200),
            load_attempts: 4,
        }
    }

    fn emfile() -> std::io::Error {
        std::io::Error::from_raw_os_error(rustix::io::Errno::MFILE.raw_os_error())
    }

    #[test]
    fn high_water_boundary() {
        assert!(under_high_water(203, 256, 80));
        assert!(!under_high_water(205, 256, 80));
        assert!(under_high_water(u64::MAX / 2, u64::MAX, 80));
    }

    #[test]
    fn classifies_fd_exhaustion() {
        assert!(is_fd_exhaustion(&emfile()));
        let parse = AgentLoadError::Parse {
            path: "x.md".into(),
            source: AgentSpecParseError::Io(emfile()),
        };
        assert!(parse.is_fd_exhaustion());
        assert!(!AgentLoadError::NotFound("x".into()).is_fd_exhaustion());
        let enoent = AgentLoadError::Io {
            path: "d".into(),
            source: std::io::Error::from(std::io::ErrorKind::NotFound),
        };
        assert!(!enoent.is_fd_exhaustion());
    }

    #[test]
    fn fd_usage_sees_every_held_descriptor() {
        use std::os::fd::AsRawFd;
        let (_, limit) = fd_usage().expect("fd usage available on unix");
        assert!(limit > 0);
        let dir = tempfile::tempdir().unwrap();
        let held: Vec<_> = (0..16)
            .map(|i| std::fs::File::create(dir.path().join(i.to_string())).unwrap())
            .collect();
        // Membership, not a before/after delta: parallel tests open and
        // close descriptors concurrently, but these 16 stay open throughout.
        let open = open_fds().unwrap();
        for f in &held {
            let fd = f.as_raw_fd() as u64;
            assert!(open.contains(&fd), "fd {fd} missing from {open:?}");
        }
        assert!(fd_usage().unwrap().0 >= 16);
    }

    #[tokio::test]
    async fn admits_immediately_under_high_water() {
        let waited = admit_with("a", &fast(), || Some((10, 256)), || false).await;
        assert!(waited < Duration::from_millis(50));
    }

    #[tokio::test]
    async fn waits_until_pressure_eases() {
        let calls = AtomicU32::new(0);
        admit_with(
            "a",
            &fast(),
            || {
                let n = calls.fetch_add(1, Ordering::SeqCst);
                Some((if n < 3 { 250 } else { 100 }, 256))
            },
            || false,
        )
        .await;
        assert_eq!(calls.load(Ordering::SeqCst), 4);
    }

    #[tokio::test]
    async fn gives_up_pacing_after_max_wait() {
        let waited = admit_with("a", &fast(), || Some((256, 256)), || false).await;
        assert!(waited >= Duration::from_millis(200));
        assert!(waited < Duration::from_secs(5));
    }

    #[tokio::test]
    async fn unknown_usage_admits() {
        let waited = admit_with("a", &fast(), || None, || false).await;
        assert!(waited < Duration::from_millis(50));
    }

    #[tokio::test]
    async fn retries_emfile_then_succeeds() {
        let calls = AtomicU32::new(0);
        let out = retry_on_fd_exhaustion(
            "a",
            &fast(),
            || false,
            || {
                if calls.fetch_add(1, Ordering::SeqCst) < 2 {
                    Err(AgentLoadError::Io {
                        path: "d".into(),
                        source: emfile(),
                    })
                } else {
                    Ok(7)
                }
            },
        )
        .await;
        assert_eq!(out.unwrap(), 7);
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn does_not_retry_other_errors_and_caps_attempts() {
        let calls = AtomicU32::new(0);
        let out: Result<(), _> = retry_on_fd_exhaustion(
            "a",
            &fast(),
            || false,
            || {
                calls.fetch_add(1, Ordering::SeqCst);
                Err(AgentLoadError::NotFound("a".into()))
            },
        )
        .await;
        assert!(out.is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        let calls = AtomicU32::new(0);
        let out: Result<(), _> = retry_on_fd_exhaustion(
            "a",
            &fast(),
            || false,
            || {
                calls.fetch_add(1, Ordering::SeqCst);
                Err(AgentLoadError::Io {
                    path: "d".into(),
                    source: emfile(),
                })
            },
        )
        .await;
        assert!(out.unwrap_err().is_fd_exhaustion());
        assert_eq!(calls.load(Ordering::SeqCst), 4);
    }

    #[tokio::test]
    async fn grows_the_limit_instead_of_waiting() {
        // Probe reflects the grown limit: 250/256 is over the mark, 250/512
        // is under it — admitted without a single sleep.
        let limit = AtomicU64::new(256);
        let grows = AtomicU32::new(0);
        let waited = admit_with(
            "a",
            &Policy {
                initial_backoff: Duration::from_secs(10),
                ..fast()
            },
            || Some((250, limit.load(Ordering::SeqCst))),
            || {
                grows.fetch_add(1, Ordering::SeqCst);
                limit.store(512, Ordering::SeqCst);
                true
            },
        )
        .await;
        assert_eq!(grows.load(Ordering::SeqCst), 1);
        assert!(waited < Duration::from_secs(1));
    }

    #[test]
    fn startup_candidates_default_and_requested() {
        // Default target, capped at the effective hard limit.
        assert_eq!(
            startup_candidates(Some(256), Some(245_760), None),
            vec![10_240]
        );
        assert_eq!(startup_candidates(Some(256), None, None), vec![10_240]);
        assert_eq!(
            startup_candidates(Some(256), Some(4_096), None),
            vec![4_096]
        );
        // Requested goes first (capped), default as the fallback.
        assert_eq!(
            startup_candidates(Some(256), Some(245_760), Some(50_000)),
            vec![50_000, 10_240]
        );
        assert_eq!(
            startup_candidates(Some(256), Some(245_760), Some(999_999_999)),
            vec![245_760, 10_240]
        );
        assert_eq!(
            startup_candidates(Some(256), Some(65_536), Some(2_048)),
            vec![2_048]
        );
        // Never lowers.
        assert!(startup_candidates(Some(20_000), None, None).is_empty());
        assert!(startup_candidates(Some(20_000), None, Some(1_024)).is_empty());
        assert!(startup_candidates(None, None, Some(1)).is_empty());
    }

    #[test]
    fn next_grow_doubles_within_caps() {
        assert_eq!(next_grow(10_240, None, None), Some(20_480));
        assert_eq!(next_grow(10_240, Some(16_000), None), Some(16_000));
        assert_eq!(next_grow(10_240, None, Some(12_000)), Some(12_000));
        assert_eq!(next_grow(16_000, Some(16_000), None), None);
        assert_eq!(next_grow(12_000, None, Some(12_000)), None);
    }

    #[test]
    fn configure_raises_real_limit_and_reports_the_effective_value() {
        let (before, eff_hard, _) = limits();
        let report = configure(None);
        assert_eq!(report.before, before);
        if let Some(b) = before {
            let floor = eff_hard.map_or(DEFAULT_TARGET, |h| h.min(DEFAULT_TARGET));
            assert!(
                report.after.is_some_and(|a| a >= floor.max(b)),
                "{report:?}"
            );
        }
        // Reported value is what the OS will actually enforce.
        assert_eq!(report.after, limits().0);
        if let (Some(a), Some(h)) = (report.after, eff_hard) {
            assert!(a <= h);
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_cap_is_read() {
        let cap = os_per_process_cap().expect("kern.maxfilesperproc readable");
        assert!(cap >= 1_024, "{cap}");
    }
}
