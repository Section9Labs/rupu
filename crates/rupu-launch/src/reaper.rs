//! Stopping and recording children nobody is watching (spec W7 §3.6). A
//! parent that ends normally stops its unjoined children itself
//! (`Launcher::finish`); one that crashed leaves its process children running
//! with nobody to join them, and one stopped by a signal records them
//! `interrupted` on its way out ([`stop_live_children`]).
//!
//! A crashed parent's child is recorded as its own records say it ended, or,
//! when it hadn't, `interrupted`: the run that continues the parent continues
//! it ([`interrupt_orphans`]). It is recorded `abandoned` instead once
//! nothing can continue it any more: its parent's own run, or the run at the
//! root of its launch tree, ended for good ([`reap_orphaned_children`]), its
//! workflow run ended ([`end_interrupted_of`]), or its parent's attempt was
//! started over ([`abandon_unfinished`]).
//!
//! Only a process its recorded pid and start time say is the child is ever
//! signalled ([`Recorded::leader`]), and only while its record has no end: a
//! child recorded `interrupted` was stopped already.
//!
//! Blocking (file IO, and up to the grace when a group needs winding down):
//! call it from `spawn_blocking` in async code.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, RwLock, RwLockReadGuard};
use std::time::{Duration, Instant};

use rupu_orchestrator::{RunStatus, RunStore, AGENT_RUN_PREFIX};
use rupu_tools::{ChildKind, ChildState};
use rupu_transcript::StopCause;
use tokio_util::sync::CancellationToken;

use crate::identity::{below, own_end, Leader, Own, Recorded};
use crate::ledger::{self, ChildEntry, Finished, LedgerLine, Started};
use crate::proc::{
    group_carries_env, group_has_members, is_zombie, kill_group, pid_is_running, process_alive,
};

/// How often the grace re-checks whether the signalled groups are gone.
const GRACE_POLL: Duration = Duration::from_millis(25);

fn alive(pid: u32) -> bool {
    pid_is_running(pid) && !is_zombie(pid)
}

/// Stop every unfinished child in `dir`'s ledger — all of them when `force`
/// (an operator's hard stop), else only those whose launching process has
/// exited — and record each `abandoned` with `why`. Returns the handles it
/// stopped.
pub fn stop_children(dir: &Path, force: bool, grace: Duration, why: &str) -> Vec<String> {
    stop_in(
        &[dir.to_path_buf()],
        force,
        ChildState::Abandoned,
        grace,
        why,
    )
    .into_iter()
    .map(|(_, h)| h)
    .collect()
}

/// The ledgers of runs in this process that launched children: what
/// [`stop_live_children`] stops when the process is told to go.
static LIVE: Mutex<BTreeSet<PathBuf>> = Mutex::new(BTreeSet::new());

fn live() -> std::sync::MutexGuard<'static, BTreeSet<PathBuf>> {
    LIVE.lock().unwrap_or_else(|e| e.into_inner())
}

pub(crate) fn track(dir: &Path) {
    live().insert(dir.to_path_buf());
}

pub(crate) fn untrack(dir: &Path) {
    live().remove(dir);
}

/// Held shared across a child's spawn and its ledger lines; the stopper
/// takes it exclusively to set the stop, so every child spawned before the
/// stop is in a ledger by the time the stopper reads them.
static SPAWNING: RwLock<()> = RwLock::new(());

/// Cancelled once this process is being stopped by a signal: from then on
/// no child is spawned, and a child waiting for a slot ends `interrupted`.
static STOP: Mutex<Option<CancellationToken>> = Mutex::new(None);

pub(crate) fn stop_token() -> CancellationToken {
    STOP.lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_or_insert_with(CancellationToken::new)
        .clone()
}

/// Whether this process is being stopped by a signal: what its runs leave
/// unfinished is continued with them, so it is recorded `interrupted`.
pub(crate) fn stopping() -> bool {
    stop_token().is_cancelled() || rupu_providers::credential_writes::terminating()
}

/// Leave to spawn a child: `None` once the process is stopping. Hold it
/// until the child's lines are written.
pub(crate) fn spawn_gate() -> Option<RwLockReadGuard<'static, ()>> {
    let gate = SPAWNING.read().unwrap_or_else(|e| e.into_inner());
    (!stop_token().is_cancelled()).then_some(gate)
}

fn begin_stopping() {
    let _held = SPAWNING.write().unwrap_or_else(|e| e.into_inner());
    stop_token().cancel();
}

/// Lifts a [`stop_live_children`] in a test process that goes on running
/// launches afterwards.
#[doc(hidden)]
pub fn reset_stopping() {
    *STOP.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// Stop every unfinished child the runs in this process launched — SIGTERM
/// to each group, at most `grace`, SIGKILL — and record each `interrupted`:
/// the run that continues its parent continues it. For a process that is
/// itself being stopped by a signal: its launcher never reaches `finish`,
/// and its children, each in its own process group, would otherwise outlive
/// it. Nothing is spawned once it starts (a child waiting for a slot ends
/// `interrupted`, a launch is refused), and it stops again until no child is
/// left or the grace is spent. Blocking; safe to call from a plain thread.
pub fn stop_live_children(grace: Duration, why: &str) -> Vec<(PathBuf, String)> {
    begin_stopping();
    let deadline = Instant::now() + grace;
    let mut stopped = Vec::new();
    loop {
        let dirs: Vec<PathBuf> = live().iter().cloned().collect();
        let left = deadline.saturating_duration_since(Instant::now());
        let round = stop_in(&dirs, true, ChildState::Interrupted, left, why);
        let done = round.is_empty() || Instant::now() >= deadline;
        stopped.extend(round);
        if done {
            return stopped;
        }
    }
}

fn stop_in(
    dirs: &[PathBuf],
    force: bool,
    end: ChildState,
    grace: Duration,
    why: &str,
) -> Vec<(PathBuf, String)> {
    let targets = dirs
        .iter()
        .flat_map(|dir| ledger::read(dir).into_iter().map(move |c| (dir.clone(), c)))
        .filter(|(_, c)| c.finished.is_none() && c.continued_by.is_none())
        .filter(|(_, c)| force || parent_gone(&c.started))
        .map(|(dir, entry)| {
            let end = Finished::new(&entry.started.handle, end, Some(why.to_string()));
            Target::stop(dir, &entry, end)
        })
        .collect();
    stop(targets, grace)
}

/// Whether the process that launched `s` has exited (a pid now running
/// another process counts as exited).
fn parent_gone(s: &Started) -> bool {
    !process_alive(s.parent_pid, s.parent_start)
}

/// The `RUPU_HOME` a ledger directory is under: every ledger is
/// `<global>/runs/<id>` or `<global>/agentiflows/<id>`.
fn global_of(dir: &Path) -> &Path {
    dir.parent().and_then(Path::parent).unwrap_or(dir)
}

/// One child to record, and the group to stop for it.
struct Target {
    dir: PathBuf,
    end: Finished,
    group: Option<(u32, Standing)>,
}

impl Target {
    /// Record `end` for `entry`, stopping its process if it has one still
    /// running.
    fn stop(dir: PathBuf, entry: &ChildEntry, end: Finished) -> Self {
        let group = entry
            .finished
            .is_none()
            .then(|| group_of(&entry.started))
            .flatten();
        Self { dir, end, group }
    }
}

/// Whether the group `g` a stopped child leaves is still standing: its
/// leader, or, once the leader has exited, any member. `Marked`: the leader
/// is gone and a member carried the child's run marker, which is checked
/// again before the SIGKILL.
enum Standing {
    Leader,
    Members,
    Marked(String),
}

/// The group to signal for `s`, if any: its own while its recorded pid and
/// start time say the process is still the child ([`Recorded::leader`]),
/// alive or an unreaped zombie (which keeps the group's id reserved, so its
/// members are the child's). Once the child's process is gone, its group's
/// id may have been taken by a stranger's: the group is signalled only while
/// a member carries the child's run marker (`RUPU_RUN_ID=<its run>`, set on
/// its command and inherited by everything it spawned). One that can't be
/// tied to the child so is left alone.
fn group_of(s: &Started) -> Option<(u32, Standing)> {
    let child = Recorded::of(s)?;
    match child.leader() {
        Leader::Live => Some((child.pid, Standing::Leader)),
        Leader::Zombie => group_has_members(child.pid).then_some((child.pid, Standing::Members)),
        Leader::Gone => {
            let marker = format!("{}={}", rupu_tools::RUN_ID_ENV, s.run_id);
            if group_carries_env(child.pid, &marker) {
                return Some((child.pid, Standing::Marked(marker)));
            }
            if group_has_members(child.pid) {
                tracing::warn!(
                    handle = %s.handle,
                    pgid = child.pid,
                    "a crashed child's process group is still standing, but nothing in it carries its run marker; it is not signalled"
                );
            }
            None
        }
    }
}

/// Record each target's end, then stop the groups — SIGTERM, at most
/// `grace` for them all, SIGKILL to what is left. Each end is recorded
/// before any signal, so a watcher that sees the child die of it can't
/// record it first as a crash, and an end its own run records later reads
/// as caused by the stop.
fn stop(targets: Vec<Target>, grace: Duration) -> Vec<(PathBuf, String)> {
    let mut groups = Vec::new();
    let mut recorded = Vec::new();
    for t in targets {
        ledger::append(&t.dir, &LedgerLine::Finished(t.end.clone()));
        groups.extend(t.group);
        recorded.push((t.dir, t.end.handle));
    }
    for &(g, _) in &groups {
        crate::proc::terminate_group(g);
    }
    let standing = |(g, s): &(u32, Standing)| match s {
        Standing::Leader => alive(*g),
        Standing::Members | Standing::Marked(_) => group_has_members(*g),
    };
    let deadline = Instant::now() + grace;
    while groups.iter().any(standing) && Instant::now() < deadline {
        std::thread::sleep(GRACE_POLL);
    }
    for group in &groups {
        // A marked group that emptied during the grace may have had its id
        // taken since: it is still the child's only while marked.
        let ours = match &group.1 {
            Standing::Marked(marker) => group_carries_env(group.0, marker),
            _ => true,
        };
        if standing(group) && ours {
            kill_group(group.0);
        }
    }
    recorded
}

/// What to record for `c`, unfinished in `dir`'s ledger with its parent's
/// process gone: `interrupted` while it runs (it is stopped) or when it died
/// before its run ended — `abandoned` instead when `for_good` — else the end
/// its own records give.
fn orphan(dir: &Path, c: &ChildEntry, for_good: bool, why: &str) -> Target {
    let s = &c.started;
    let group = group_of(s);
    let own = match group {
        Some((_, Standing::Leader)) => Own::Unended,
        _ => own_end(global_of(dir), s, None),
    };
    let end = match own {
        Own::Ended(end) => end,
        Own::Unended | Own::NotStarted => {
            let state = if for_good {
                ChildState::Abandoned
            } else {
                ChildState::Interrupted
            };
            Finished::new(&s.handle, state, Some(why.to_string()))
        }
    };
    Target {
        dir: dir.to_path_buf(),
        end,
        group,
    }
}

/// The first step of continuing `dir`'s run: the children its process left
/// without an end when it died (a crash records nothing) are stopped if
/// still running and recorded `interrupted`, or as their own records say
/// they ended. Returns the ledger as it then reads. A child whose launching
/// process is still running is left alone.
pub fn interrupt_orphans(dir: &Path, grace: Duration) -> Vec<ChildEntry> {
    let targets = ledger::read(dir)
        .iter()
        .filter(|c| c.finished.is_none() && c.continued_by.is_none() && parent_gone(&c.started))
        .map(|c| orphan(dir, c, false, "its parent's process ended before it did"))
        .collect();
    stop(targets, grace);
    ledger::read(dir)
}

/// Nested children are at most this many levels below (the launch depth
/// limit, with room to spare).
const MAX_NESTING: usize = 8;

/// Abandon what nothing will continue, down from `dirs`: every child
/// without a final end — interrupted, or unfinished with its launching
/// process gone — and, below each, its own children.
struct Abandon<'a> {
    why: &'a str,
    seen: BTreeSet<(PathBuf, String)>,
    targets: Vec<Target>,
}

impl<'a> Abandon<'a> {
    fn new(why: &'a str) -> Self {
        Self {
            why,
            seen: BTreeSet::new(),
            targets: Vec::new(),
        }
    }

    /// `c`, in `dir`'s ledger, and everything below it.
    fn child(&mut self, dir: &Path, c: &ChildEntry, depth: usize) {
        if !self
            .seen
            .insert((dir.to_path_buf(), c.started.handle.clone()))
        {
            return;
        }
        let target = if c.is_interrupted() {
            Target {
                dir: dir.to_path_buf(),
                end: Finished::new(
                    &c.started.handle,
                    ChildState::Abandoned,
                    Some(self.why.to_string()),
                ),
                group: None,
            }
        } else {
            orphan(dir, c, true, self.why)
        };
        self.targets.push(target);
        if depth < MAX_NESTING {
            for below in below(global_of(dir), c.started.kind, &c.started.run_id) {
                self.dir(&below, depth + 1);
            }
        }
    }

    fn dir(&mut self, dir: &Path, depth: usize) {
        for c in ledger::read(dir) {
            let orphaned = c.finished.is_none() && parent_gone(&c.started);
            if c.continued_by.is_none() && (c.is_interrupted() || orphaned) {
                self.child(dir, &c, depth);
            }
        }
    }

    fn finish(self, grace: Duration) -> Vec<(PathBuf, String)> {
        stop(self.targets, grace)
    }
}

/// Record every child in `dir`'s ledger without a final end — interrupted,
/// or unfinished with its launching process gone — `abandoned` with `why`
/// (or as its own records say it ended), stopping any still running, and
/// the same for each one's own children: nothing will continue them.
/// Returns what it recorded.
pub fn abandon_unfinished(dir: &Path, why: &str, grace: Duration) -> Vec<(PathBuf, String)> {
    let mut abandon = Abandon::new(why);
    abandon.dir(dir, 0);
    abandon.finish(grace)
}

/// [`abandon_unfinished`] below a child that ended for good: its own run's
/// children, or a workflow child's attempts'.
pub fn abandon_below(
    global: &Path,
    kind: ChildKind,
    run_id: &str,
    why: &str,
    grace: Duration,
) -> Vec<(PathBuf, String)> {
    let mut abandon = Abandon::new(why);
    for dir in below(global, kind, run_id) {
        abandon.dir(&dir, 1);
    }
    abandon.finish(grace)
}

/// The workflow run `workflow_run_id` ended for good: every child its step
/// and unit attempts (and their children, down the tree) left
/// `interrupted`, or unfinished with its launching process gone, is
/// recorded `abandoned` — none of its steps continued it, and none will. A
/// child an attempt took on is closed in the earlier attempt's ledger, and
/// judged in the one that took it on.
pub fn end_interrupted_of(
    global: &Path,
    workflow_run_id: &str,
    grace: Duration,
) -> Vec<(PathBuf, String)> {
    abandon_below(
        global,
        ChildKind::Workflow,
        workflow_run_id,
        &format!("its workflow run {workflow_run_id} ended without continuing it"),
        grace,
    )
}

/// Whether nothing can continue the children in `dir`'s ledger that `s`
/// records: an agentiflow's (a flow is never continued), or ones whose
/// parent's own run, or the run at the root of their launch tree, ended for
/// good. A `rupu run` records its run only once it ended, but one a signal
/// stopped (its failure's typed cause) is continued like a paused one; a
/// workflow run that failed can still be resumed, so only its other
/// terminal statuses count.
fn ended_for_good(global: &Path, dir: &Path, s: &Started) -> bool {
    if dir.starts_with(global.join("agentiflows")) {
        return true;
    }
    let runs = global.join("runs");
    let store = RunStore::new(runs.clone());
    let ended = |d: &Path| {
        d.parent() == Some(runs.as_path())
            && d.file_name()
                .and_then(|n| n.to_str())
                .and_then(|id| store.load(id).ok())
                .is_some_and(|r| match r.status {
                    RunStatus::Failed => {
                        r.workflow_name.starts_with(AGENT_RUN_PREFIX)
                            && r.cause.as_ref().and_then(StopCause::of).is_none()
                    }
                    status => status.is_terminal(),
                })
    };
    ended(dir) || s.tree.as_deref().is_some_and(ended)
}

/// What one [`reap_orphaned_children`] sweep did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ReapSummary {
    /// Run directories whose ledger was read.
    pub scanned: usize,
    /// `(run dir, handle)` of every child it stopped or recorded.
    pub reaped: Vec<(PathBuf, String)>,
}

/// Sweep every children ledger under `<global>/runs/*` and
/// `<global>/agentiflows/*`. A child whose launching process died is
/// stopped and recorded `interrupted` (continued with its parent), or
/// `abandoned` when nothing can continue it any more ([`ended_for_good`]),
/// or as its own records say it ended; an `interrupted` child nothing can
/// continue any more is recorded `abandoned`, with what is below it. A child
/// another run took on is skipped: its latest record is that run's.
pub fn reap_orphaned_children(global: &Path, grace: Duration) -> ReapSummary {
    let mut summary = ReapSummary::default();
    let mut ledgers: Vec<(PathBuf, Vec<ChildEntry>)> = Vec::new();
    for root in [global.join("runs"), global.join("agentiflows")] {
        let Ok(entries) = std::fs::read_dir(&root) else {
            continue;
        };
        for entry in entries.flatten() {
            let dir = entry.path();
            if !dir.join(ledger::CHILDREN_FILE).is_file() {
                continue;
            }
            summary.scanned += 1;
            ledgers.push((dir.clone(), ledger::read(&dir)));
        }
    }
    let mut abandon = Abandon::new("nothing can continue its parent any more");
    let mut orphans = Vec::new();
    for (dir, entries) in &ledgers {
        for c in entries.iter().filter(|c| c.continued_by.is_none()) {
            let orphaned = c.finished.is_none() && parent_gone(&c.started);
            if !orphaned && !c.is_interrupted() {
                continue;
            }
            if ended_for_good(global, dir, &c.started) {
                abandon.child(dir, c, 0);
            } else if orphaned {
                orphans.push((dir, c));
            }
        }
    }
    let interrupt = orphans
        .into_iter()
        .filter(|(dir, c)| !abandon.seen.contains(&((*dir).clone(), c.started.handle.clone())))
        .map(|(dir, c)| {
            orphan(
                dir,
                c,
                false,
                "orphaned: the process that launched it is gone; the run that continues its parent continues it",
            )
        })
        .collect();
    summary.reaped = abandon.finish(grace);
    summary.reaped.extend(stop(interrupt, grace));
    summary
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::BufRead;
    use std::os::unix::process::CommandExt;

    const LINGER: &str = "RUPU_LAUNCH_TEST_LINGER";

    /// Not a test: the process a crashed child leaves behind. Runs only when
    /// re-invoked by [`crashed_child`], as this very binary, whose
    /// environment the platform lets another process read (macOS hides that
    /// of its own binaries, `/bin/sleep` among them).
    #[test]
    #[ignore]
    fn linger() {
        if std::env::var_os(LINGER).is_some() {
            std::thread::sleep(Duration::from_secs(60));
        }
    }

    /// A child that crashed with `marker` in its environment: its process
    /// (the group's leader) is gone, reaped, and what it started is still
    /// running in its group. Its `started` line, as its dead parent recorded
    /// it, and the leftover's pid.
    fn crashed_child(handle: &str, run_id: &str, marker: &str) -> (LedgerLine, u32) {
        let exe = std::env::current_exe().unwrap();
        let mut leader = std::process::Command::new("/bin/sh")
            .args([
                "-c",
                "\"$0\" --exact reaper::tests::linger --ignored >/dev/null 2>&1 & echo $!",
            ])
            .arg(exe)
            .env(rupu_tools::RUN_ID_ENV, marker)
            .env(LINGER, "1")
            .process_group(0)
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let pid = leader.id();
        let start = crate::proc::start_stamp(pid);
        let mut line = String::new();
        std::io::BufReader::new(leader.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        leader.wait().unwrap();
        let started = LedgerLine::Started(Started {
            pid: Some(pid),
            pgid: Some(pid),
            proc_start: start,
            ..Started::new(handle, ChildKind::Agent, "recon", run_id, 0x7fff_fffe, 1)
        });
        (started, line.trim().parse().unwrap())
    }

    /// A crashed child's leftover process, still in its group, is stopped by
    /// the sweep once it carries the child's run marker; a leaderless group
    /// whose processes carry no such marker (here: another run's) is a
    /// stranger's and is left alone. Either child is recorded.
    #[test]
    fn a_crashed_childs_marked_leftovers_are_stopped_and_strangers_left_alone() {
        let global = tempfile::tempdir().unwrap();
        let dir = global.path().join("runs").join("run_parent");
        let (ours, leftover) = crashed_child("ch_ours", "run_crashed", "run_crashed");
        let (theirs, stranger) = crashed_child("ch_theirs", "run_other", "run_other2");
        ledger::append(&dir, &ours);
        ledger::append(&dir, &theirs);
        assert!(alive(leftover) && alive(stranger));

        let summary = reap_orphaned_children(global.path(), Duration::from_secs(5));

        let deadline = Instant::now() + Duration::from_secs(5);
        while alive(leftover) && Instant::now() < deadline {
            std::thread::sleep(GRACE_POLL);
        }
        let stranger_alive = alive(stranger);
        crate::proc::kill_group(stranger);
        assert!(!alive(leftover), "the crashed child's leftover was stopped");
        assert!(stranger_alive, "a stranger's group was signalled");
        assert_eq!(summary.reaped.len(), 2, "{summary:?}");
        let states: Vec<_> = ledger::read(&dir).iter().map(ChildEntry::state).collect();
        assert_eq!(states, [ChildState::Interrupted, ChildState::Interrupted]);
    }

    fn started(handle: &str, parent_pid: u32, pgid: Option<u32>) -> LedgerLine {
        LedgerLine::Started(Started {
            pid: pgid,
            pgid,
            proc_start: pgid.and_then(crate::proc::start_stamp),
            ..Started::new(handle, ChildKind::Agent, "recon", "run_1", parent_pid, 1)
        })
    }

    /// A crashed `rupu run`'s process child is stopped and recorded
    /// `interrupted`, for `rupu run --continue` to continue; a live parent's
    /// child is left alone, and a second sweep has nothing to do.
    #[test]
    fn a_dead_parents_process_child_is_stopped_and_recorded_interrupted() {
        let global = tempfile::tempdir().unwrap();
        let dir = global.path().join("runs").join("run_parent");
        let mut child = std::process::Command::new("/bin/sh")
            .args(["-c", "sleep 30"])
            .process_group(0)
            .spawn()
            .unwrap();
        // A parent pid that can't be running.
        ledger::append(
            &dir,
            &started("ch_dead_parent", 0x7fff_fffe, Some(child.id())),
        );
        ledger::append(&dir, &started("ch_live_parent", std::process::id(), None));

        let summary = reap_orphaned_children(global.path(), Duration::from_secs(5));
        assert_eq!(
            summary.reaped,
            vec![(dir.clone(), "ch_dead_parent".to_string())]
        );
        child.wait().unwrap();
        let states: Vec<_> = ledger::read(&dir).iter().map(ChildEntry::state).collect();
        assert_eq!(states, [ChildState::Interrupted, ChildState::Running]);
        assert!(reap_orphaned_children(global.path(), Duration::ZERO)
            .reaped
            .is_empty());
    }
}
