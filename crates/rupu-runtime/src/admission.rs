//! Process-wide admission control for concurrent agent jobs.
//!
//! A wide fan-out or a `split:` DAG can dispatch an unbounded number of agent
//! runs at once (the orchestrator's per-step `max_parallel:` defaults to 1 but
//! is uncapped, and the DAG scheduler's `max_concurrency:` defaults to
//! effectively unbounded). Each concurrent run holds a conversation in memory
//! and spawns tool subprocesses, so without a machine-level bound a big run can
//! drive the host out of memory. This module is that bound — two layers:
//!
//! 1. **A hard ceiling** ([`Admission`] holds a [`Semaphore`]) on how many
//!    local agent jobs run at once, enforced regardless of what a workflow
//!    declares. It bounds the blast radius.
//! 2. **A memory watchdog** that holds a new job back while available system
//!    memory is below a headroom floor, so an in-flight allocation spike can't
//!    push the machine over the edge. It is the adaptive safety net.
//!
//! The gate is a write-once process global ([`configure`] / [`global`]).
//! **Unconfigured, it is fully open** — every [`acquire`] returns immediately —
//! so library callers and tests are unaffected until a run entry point opts in.
//! The [`Admission`] type itself is constructible with explicit limits and an
//! injected [`MemoryProbe`] for direct unit testing, independent of the global.

use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use rupu_config::runtime_config::RuntimeConfig;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

const MB: u64 = 1024 * 1024;
const GB: u64 = 1024 * MB;

/// Default ceiling bounds: `clamp(total_RAM_GB / 4, 4, 128)`.
const CEILING_MIN: usize = 4;
const CEILING_MAX: usize = 128;
const CEILING_BYTES_PER_JOB: u64 = 4 * GB; // RAM/4 ⇒ ~4 GB budgeted per job

/// Default watchdog headroom: `max(total_RAM * 0.10, 4 GB)`.
const HEADROOM_FRACTION: f64 = 0.10;
const HEADROOM_FLOOR_BYTES: u64 = 4 * GB;

/// How often the watchdog re-reads memory while a job is held back, and how
/// long it will hold a job before failing open (so a machine whose memory
/// never recovers still makes forward progress, bounded by the ceiling).
const WATCHDOG_POLL: Duration = Duration::from_millis(500);
const WATCHDOG_MAX_WAIT: Duration = Duration::from_secs(60);

/// How long a [`SystemMemoryProbe`] reading of *available* memory is reused
/// before re-sampling, so a burst of dispatches shares one reading instead of
/// spawning a `vm_stat` per job on macOS.
const PROBE_TTL: Duration = Duration::from_millis(1000);

/// A source of system memory facts. Injected so the watchdog is testable
/// without actually exhausting RAM.
pub trait MemoryProbe: Send + Sync {
    /// Currently-available system memory in bytes (free plus what the OS can
    /// reclaim without paging out live data), or `None` if it cannot be
    /// determined on this platform — in which case the watchdog fails open.
    fn available_bytes(&self) -> Option<u64>;

    /// Total physical memory in bytes, or `None` if unknown. Used only to
    /// derive defaults.
    fn total_bytes(&self) -> Option<u64>;
}

/// Outcome of an [`Admission::acquire`]: the permit plus how long (and why) the
/// job was held back, so the caller can surface a throttle as a run warning.
pub struct JobPermit {
    // Dropped (released) when the permit is dropped. `None` when no ceiling is
    // configured.
    _permit: Option<OwnedSemaphorePermit>,
    /// How long the call waited for a free slot under the ceiling.
    pub waited_for_slot: Duration,
    /// How long the call waited for system memory to reach the headroom floor.
    pub waited_for_memory: Duration,
    /// The memory watchdog gave up waiting and let the job through anyway.
    pub memory_timed_out: bool,
}

impl JobPermit {
    /// Whether this acquisition was delayed by either limit.
    pub fn throttled(&self) -> bool {
        !self.waited_for_slot.is_zero() || !self.waited_for_memory.is_zero()
    }
}

/// The admission gate: a hard concurrency ceiling plus a memory watchdog.
pub struct Admission {
    /// `None` ⇒ no ceiling (fully open).
    ceiling: Option<Arc<Semaphore>>,
    /// The configured ceiling capacity (what [`ceiling`](Self::ceiling)
    /// reports), independent of how many permits are currently free.
    capacity: Option<usize>,
    /// Minimum available system memory before a new job is admitted. `0`
    /// disables the watchdog.
    min_free_bytes: u64,
    probe: Arc<dyn MemoryProbe>,
    poll: Duration,
    max_wait: Duration,
}

impl Admission {
    /// A fully-open gate: no ceiling, no watchdog. Every acquire is immediate.
    pub fn unbounded() -> Self {
        Self {
            ceiling: None,
            capacity: None,
            min_free_bytes: 0,
            probe: Arc::new(SystemMemoryProbe::default()),
            poll: WATCHDOG_POLL,
            max_wait: WATCHDOG_MAX_WAIT,
        }
    }

    /// A gate with an explicit ceiling and memory floor, backed by the real
    /// system probe. `max_jobs == 0` ⇒ no ceiling; `min_free_bytes == 0` ⇒ no
    /// watchdog.
    pub fn with_limits(max_jobs: usize, min_free_bytes: u64) -> Self {
        Self::with_probe(
            max_jobs,
            min_free_bytes,
            Arc::new(SystemMemoryProbe::default()),
            WATCHDOG_POLL,
            WATCHDOG_MAX_WAIT,
        )
    }

    /// As [`with_limits`](Self::with_limits) but with an injected probe and
    /// watchdog timings — the unit-test seam.
    pub fn with_probe(
        max_jobs: usize,
        min_free_bytes: u64,
        probe: Arc<dyn MemoryProbe>,
        poll: Duration,
        max_wait: Duration,
    ) -> Self {
        Self {
            ceiling: (max_jobs > 0).then(|| Arc::new(Semaphore::new(max_jobs))),
            capacity: (max_jobs > 0).then_some(max_jobs),
            min_free_bytes,
            probe,
            poll,
            max_wait,
        }
    }

    /// Build a gate from `[runtime]` config, applying env overrides and
    /// RAM-derived defaults. Env wins over config wins over default.
    pub fn from_config(cfg: &RuntimeConfig) -> Self {
        let probe = Arc::new(SystemMemoryProbe::default());
        let total = probe.total_bytes();
        let max_jobs = env_usize("RUPU_MAX_CONCURRENT_JOBS")
            .or(cfg.max_concurrent_jobs)
            .unwrap_or_else(|| default_ceiling(total));
        let min_free_bytes = env_u64("RUPU_MIN_FREE_MEMORY_MB")
            .or(cfg.min_free_memory_mb)
            .map(|mb| mb.saturating_mul(MB))
            .unwrap_or_else(|| default_min_free(total));
        Self::with_probe(max_jobs, min_free_bytes, probe, WATCHDOG_POLL, WATCHDOG_MAX_WAIT)
    }

    /// The configured ceiling capacity, or `None` when unbounded.
    pub fn ceiling(&self) -> Option<usize> {
        self.capacity
    }

    /// The watchdog's memory floor in bytes (`0` when disabled).
    pub fn min_free_bytes(&self) -> u64 {
        self.min_free_bytes
    }

    /// Acquire a slot for one local agent job: block until a ceiling permit is
    /// free, then until system memory is above the headroom floor (bounded by
    /// `max_wait`). The returned [`JobPermit`] must be held for the job's
    /// lifetime; dropping it releases the slot.
    pub async fn acquire(&self) -> JobPermit {
        // Only count a slot wait when a permit was not immediately free, so an
        // uncontended acquire reports zero (not measurement noise).
        let (permit, waited_for_slot) = match &self.ceiling {
            Some(sem) => match Arc::clone(sem).try_acquire_owned() {
                Ok(p) => (Some(p), Duration::ZERO),
                Err(_) => {
                    let start = Instant::now();
                    let p = Arc::clone(sem)
                        .acquire_owned()
                        .await
                        .expect("admission semaphore is never closed");
                    (Some(p), start.elapsed())
                }
            },
            None => (None, Duration::ZERO),
        };

        let (waited_for_memory, memory_timed_out) = self.await_memory().await;

        JobPermit {
            _permit: permit,
            waited_for_slot,
            waited_for_memory,
            memory_timed_out,
        }
    }

    /// Hold until available memory ≥ the floor, or `max_wait` elapses. Returns
    /// how long it waited and whether it timed out. Fails open (no wait) when
    /// the watchdog is disabled or the probe cannot read memory.
    async fn await_memory(&self) -> (Duration, bool) {
        if self.min_free_bytes == 0 {
            return (Duration::ZERO, false);
        }
        // `slept` accumulates only real backoff, so a reading that passes (or
        // is unknown) on the first look reports a zero wait, not noise.
        let start = Instant::now();
        let mut slept = Duration::ZERO;
        loop {
            match self.sample_available().await {
                // Can't tell → fail open rather than stall forever.
                None => return (slept, false),
                Some(avail) if avail >= self.min_free_bytes => return (slept, false),
                Some(_) => {
                    if start.elapsed() >= self.max_wait {
                        return (slept, true);
                    }
                    tokio::time::sleep(self.poll).await;
                    slept = start.elapsed();
                }
            }
        }
    }

    /// Read available memory off the executor (the real probe may shell out).
    async fn sample_available(&self) -> Option<u64> {
        let probe = Arc::clone(&self.probe);
        tokio::task::spawn_blocking(move || probe.available_bytes())
            .await
            .unwrap_or(None)
    }
}

/// `clamp(total_RAM_GB / 4, 4, 128)`; falls back to `available_parallelism`
/// (clamped the same) when total memory is unknown.
fn default_ceiling(total_bytes: Option<u64>) -> usize {
    let raw = match total_bytes {
        Some(total) => (total / CEILING_BYTES_PER_JOB) as usize,
        None => std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(CEILING_MIN),
    };
    raw.clamp(CEILING_MIN, CEILING_MAX)
}

/// `max(total_RAM * 0.10, 4 GB)`; the floor alone when total is unknown.
fn default_min_free(total_bytes: Option<u64>) -> u64 {
    let proportional = total_bytes
        .map(|t| (t as f64 * HEADROOM_FRACTION) as u64)
        .unwrap_or(0);
    proportional.max(HEADROOM_FLOOR_BYTES)
}

fn env_usize(key: &str) -> Option<usize> {
    std::env::var(key).ok()?.trim().parse().ok()
}

fn env_u64(key: &str) -> Option<u64> {
    std::env::var(key).ok()?.trim().parse().ok()
}

// ---- Process-global gate -------------------------------------------------

static GLOBAL: OnceLock<Admission> = OnceLock::new();

/// Configure the process-global gate from `[runtime]` config. Idempotent — the
/// first call wins (a process has one config); later calls are ignored. Call
/// once at a run entry point (`rupu run`, the `cp serve` resume worker).
pub fn configure(cfg: &RuntimeConfig) {
    let _ = GLOBAL.set(Admission::from_config(cfg));
}

/// The process-global gate. Unconfigured ⇒ a fully-open [`Admission::unbounded`]
/// gate, so nothing is throttled until [`configure`] opts in.
pub fn global() -> &'static Admission {
    GLOBAL.get_or_init(Admission::unbounded)
}

/// Acquire a job slot from the process-global gate. See [`Admission::acquire`].
pub async fn acquire() -> JobPermit {
    global().acquire().await
}

// ---- Platform memory probe ----------------------------------------------

/// Reads system memory from the OS: `/proc/meminfo` on Linux, `sysctl` +
/// `vm_stat` on macOS. All pure-safe (fs reads / shelling out — the workspace
/// forbids `unsafe_code`, so raw mach/syscall calls are out). Available-memory
/// readings are cached for [`PROBE_TTL`]; total memory never changes and is
/// cached for the probe's life.
#[derive(Default)]
pub struct SystemMemoryProbe {
    total: OnceLock<Option<u64>>,
    available: Mutex<Option<(Instant, Option<u64>)>>,
}

impl MemoryProbe for SystemMemoryProbe {
    fn available_bytes(&self) -> Option<u64> {
        if let Ok(guard) = self.available.lock() {
            if let Some((at, cached)) = *guard {
                if at.elapsed() < PROBE_TTL {
                    return cached;
                }
            }
        }
        let fresh = read_available_bytes();
        if let Ok(mut guard) = self.available.lock() {
            *guard = Some((Instant::now(), fresh));
        }
        fresh
    }

    fn total_bytes(&self) -> Option<u64> {
        *self.total.get_or_init(read_total_bytes)
    }
}

#[cfg(target_os = "linux")]
fn read_total_bytes() -> Option<u64> {
    meminfo_field("MemTotal:")
}

#[cfg(target_os = "linux")]
fn read_available_bytes() -> Option<u64> {
    meminfo_field("MemAvailable:")
}

/// Parse a `/proc/meminfo` field (reported in kB) into bytes.
#[cfg(target_os = "linux")]
fn meminfo_field(name: &str) -> Option<u64> {
    let text = std::fs::read_to_string("/proc/meminfo").ok()?;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix(name) {
            let kb: u64 = rest.split_whitespace().next()?.parse().ok()?;
            return Some(kb.saturating_mul(1024));
        }
    }
    None
}

#[cfg(target_os = "macos")]
fn read_total_bytes() -> Option<u64> {
    let out = std::process::Command::new("sysctl")
        .args(["-n", "hw.memsize"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}

/// Sum the reclaimable page classes from `vm_stat` (free + inactive +
/// speculative + purgeable) × the page size it reports.
#[cfg(target_os = "macos")]
fn read_available_bytes() -> Option<u64> {
    let out = std::process::Command::new("vm_stat").output().ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);

    // Header: "Mach Virtual Memory Statistics: (page size of 16384 bytes)"
    let page_size: u64 = text
        .lines()
        .next()
        .and_then(|l| l.split("page size of ").nth(1))
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|n| n.parse().ok())
        .unwrap_or(4096);

    let pages = |name: &str| -> u64 {
        text.lines()
            .find_map(|l| l.strip_prefix(name))
            .and_then(|rest| rest.trim().trim_end_matches('.').parse::<u64>().ok())
            .unwrap_or(0)
    };
    let reclaimable = pages("Pages free:")
        + pages("Pages inactive:")
        + pages("Pages speculative:")
        + pages("Pages purgeable:");
    Some(reclaimable.saturating_mul(page_size))
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn read_total_bytes() -> Option<u64> {
    None
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn read_available_bytes() -> Option<u64> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// A probe returning a fixed available/total, counting reads.
    struct FixedProbe {
        available: Option<u64>,
        total: Option<u64>,
        reads: AtomicU64,
    }
    impl FixedProbe {
        fn new(available: Option<u64>, total: Option<u64>) -> Arc<Self> {
            Arc::new(Self {
                available,
                total,
                reads: AtomicU64::new(0),
            })
        }
    }
    impl MemoryProbe for FixedProbe {
        fn available_bytes(&self) -> Option<u64> {
            self.reads.fetch_add(1, Ordering::SeqCst);
            self.available
        }
        fn total_bytes(&self) -> Option<u64> {
            self.total
        }
    }

    /// A probe whose available memory recovers after N reads.
    struct RecoveringProbe {
        reads: AtomicU64,
        recover_after: u64,
        low: u64,
        high: u64,
    }
    impl MemoryProbe for RecoveringProbe {
        fn available_bytes(&self) -> Option<u64> {
            let n = self.reads.fetch_add(1, Ordering::SeqCst);
            Some(if n >= self.recover_after { self.high } else { self.low })
        }
        fn total_bytes(&self) -> Option<u64> {
            None
        }
    }

    #[tokio::test]
    async fn ceiling_serializes_beyond_capacity() {
        let gate = Arc::new(Admission::with_probe(
            2,
            0,
            FixedProbe::new(Some(u64::MAX), None),
            Duration::from_millis(1),
            Duration::from_secs(1),
        ));
        let p1 = gate.acquire().await;
        let p2 = gate.acquire().await;
        // A third acquire must not complete while two permits are held.
        let g = Arc::clone(&gate);
        let third = tokio::spawn(async move { g.acquire().await });
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(!third.is_finished(), "third job must wait for a free slot");
        drop(p1);
        let p3 = third.await.unwrap();
        assert!(p3.throttled(), "the third job waited for a slot");
        drop(p2);
        drop(p3);
    }

    #[tokio::test]
    async fn unbounded_gate_never_blocks() {
        let gate = Admission::unbounded();
        for _ in 0..1000 {
            let p = gate.acquire().await;
            assert!(!p.throttled());
        }
        assert_eq!(gate.ceiling(), None);
    }

    #[tokio::test]
    async fn watchdog_holds_until_memory_recovers() {
        let probe = Arc::new(RecoveringProbe {
            reads: AtomicU64::new(0),
            recover_after: 3,
            low: GB,
            high: 100 * GB,
        });
        let gate = Admission::with_probe(
            0,
            8 * GB,
            probe,
            Duration::from_millis(5),
            Duration::from_secs(5),
        );
        let p = gate.acquire().await;
        assert!(p.throttled(), "held back while memory was low");
        assert!(!p.memory_timed_out, "memory recovered before the deadline");
        assert!(p.waited_for_memory >= Duration::from_millis(10));
    }

    #[tokio::test]
    async fn watchdog_fails_open_after_max_wait() {
        let gate = Admission::with_probe(
            0,
            8 * GB,
            FixedProbe::new(Some(GB), None), // never recovers
            Duration::from_millis(5),
            Duration::from_millis(30),
        );
        let p = gate.acquire().await;
        assert!(p.memory_timed_out, "gave up and let the job through");
    }

    #[tokio::test]
    async fn watchdog_fails_open_when_probe_is_unknown() {
        let gate = Admission::with_probe(
            0,
            8 * GB,
            FixedProbe::new(None, None),
            Duration::from_millis(5),
            Duration::from_secs(10),
        );
        let p = gate.acquire().await;
        assert!(!p.throttled(), "unknown memory → fail open immediately");
    }

    #[test]
    fn default_ceiling_scales_and_clamps() {
        assert_eq!(default_ceiling(Some(256 * GB)), 64); // 256/4
        assert_eq!(default_ceiling(Some(16 * GB)), 4); // 16/4 = 4
        assert_eq!(default_ceiling(Some(8 * GB)), 4); // below min → clamp
        assert_eq!(default_ceiling(Some(2048 * GB)), 128); // above max → clamp
    }

    #[test]
    fn default_min_free_scales_and_floors() {
        assert_eq!(default_min_free(Some(256 * GB)), (256.0 * GB as f64 * 0.10) as u64);
        assert_eq!(default_min_free(Some(16 * GB)), HEADROOM_FLOOR_BYTES); // 1.6GB → floored to 4GB
        assert_eq!(default_min_free(None), HEADROOM_FLOOR_BYTES);
    }

    #[test]
    fn zero_limits_mean_open() {
        let gate = Admission::with_limits(0, 0);
        assert_eq!(gate.ceiling(), None);
        assert_eq!(gate.min_free_bytes(), 0);
    }

    // `from_config` reads two process-global env vars; serialize the tests that
    // touch them so parallel cases don't clobber each other.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn from_config_env_overrides_config() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        std::env::set_var("RUPU_MAX_CONCURRENT_JOBS", "7");
        std::env::set_var("RUPU_MIN_FREE_MEMORY_MB", "2048");
        let cfg = RuntimeConfig {
            max_concurrent_jobs: Some(99),
            min_free_memory_mb: Some(99999),
            ..Default::default()
        };
        let gate = Admission::from_config(&cfg);
        assert_eq!(gate.ceiling(), Some(7), "env wins over config for the ceiling");
        assert_eq!(gate.min_free_bytes(), 2048 * MB, "env wins over config for memory");
        std::env::remove_var("RUPU_MAX_CONCURRENT_JOBS");
        std::env::remove_var("RUPU_MIN_FREE_MEMORY_MB");
    }

    #[test]
    fn from_config_uses_config_then_default() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        std::env::remove_var("RUPU_MAX_CONCURRENT_JOBS");
        std::env::remove_var("RUPU_MIN_FREE_MEMORY_MB");
        // Config value honored when no env override.
        let cfg = RuntimeConfig {
            max_concurrent_jobs: Some(12),
            min_free_memory_mb: Some(6144),
            ..Default::default()
        };
        let gate = Admission::from_config(&cfg);
        assert_eq!(gate.ceiling(), Some(12));
        assert_eq!(gate.min_free_bytes(), 6144 * MB);

        // Empty config → RAM-derived defaults (within the documented clamps).
        let gate = Admission::from_config(&RuntimeConfig::default());
        let ceiling = gate.ceiling().expect("default ceiling is bounded");
        assert!(
            (CEILING_MIN..=CEILING_MAX).contains(&ceiling),
            "default ceiling {ceiling} within clamp"
        );
        assert!(
            gate.min_free_bytes() >= HEADROOM_FLOOR_BYTES,
            "default headroom is at least the floor"
        );
    }
}
