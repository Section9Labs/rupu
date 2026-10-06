//! `rupu-cli` integration tests that mutate process-global state — env vars
//! (`RUPU_HOME`, `RUPU_MOCK_PROVIDER_SCRIPT`, `RUPU_AUTH_FILE`, ...) and the
//! working directory — linked as one test binary.
//!
//! Those mutations reach every thread in the process and every `rupu` child
//! spawned while they are in effect, so EVERY test here holds [`ENV_LOCK`]
//! for its whole body and this binary runs one test at a time. Tests that
//! don't touch process state belong in `tests/it/`, which runs in parallel.
//! `tests/it/test_layout.rs` checks that each test here takes the lock.

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::PathBuf;

/// The one lock over process env + cwd for this binary.
pub static ENV_LOCK: ProcessStateLock = ProcessStateLock(tokio::sync::Mutex::const_new(()));

/// tokio's mutex so async tests can hold it across `.await` (sync tests
/// `blocking_lock()`), and so a panicking test releases it instead of
/// poisoning every test after it.
pub struct ProcessStateLock(tokio::sync::Mutex<()>);

impl ProcessStateLock {
    pub async fn lock(&self) -> ProcessStateGuard<'_> {
        ProcessStateGuard::new(self.0.lock().await)
    }

    pub fn blocking_lock(&self) -> ProcessStateGuard<'_> {
        ProcessStateGuard::new(self.0.blocking_lock())
    }
}

/// Holds [`ENV_LOCK`] and, when dropped (panics included), puts every env
/// var and the cwd back the way it found them. Tests here routinely end
/// with cwd inside a `TempDir` they have just deleted, or with
/// `RUPU_MOCK_PROVIDER_SCRIPT` still set; that was harmless while each file
/// was its own process, but here it would leak into the next test.
pub struct ProcessStateGuard<'a> {
    _lock: tokio::sync::MutexGuard<'a, ()>,
    env: HashMap<OsString, OsString>,
    cwd: PathBuf,
}

impl<'a> ProcessStateGuard<'a> {
    fn new(lock: tokio::sync::MutexGuard<'a, ()>) -> Self {
        Self {
            _lock: lock,
            env: std::env::vars_os().collect(),
            cwd: std::env::current_dir()
                .unwrap_or_else(|_| PathBuf::from(env!("CARGO_MANIFEST_DIR"))),
        }
    }
}

impl Drop for ProcessStateGuard<'_> {
    fn drop(&mut self) {
        for (key, _) in std::env::vars_os() {
            if !self.env.contains_key(&key) {
                std::env::remove_var(&key);
            }
        }
        for (key, value) in &self.env {
            if std::env::var_os(key).as_ref() != Some(value) {
                std::env::set_var(key, value);
            }
        }
        // Not unwrap: a failed restore must not turn into a panic while
        // unwinding from a test's own failure (that aborts the binary).
        let _ = std::env::set_current_dir(&self.cwd);
    }
}

mod accounts_sso_e2e;
mod agentiflow_run;
mod approve_resume_run_step;
mod attach_loop_persists;
mod cli_agent;
mod cli_auth;
mod cli_autoflow;
mod cli_config;
mod cli_cp_serve_bind;
mod cli_issues_multi_account;
mod cli_paths;
mod cli_provider_factory;
mod cli_repos;
mod cli_run;
mod cli_run_continue_model;
mod cli_run_fallback;
mod cli_samples;
mod cli_transcript;
mod cli_watch;
mod cli_workflow;
mod coverage_audit_cli;
mod multi_gate_approve;
mod netflow_run;
mod netflow_subprocess_live;
mod netflow_workflow;
mod policy_lock;
mod reject_mode_inheritance;
mod resume_clears_web_marker;
mod resume_recovers_interrupted;
mod run_from_file;
mod workflow_runs_no_side_effects;
