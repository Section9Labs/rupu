//! Local execution provenance for project workflow history. A worker ID alone
//! identifies neither a local execution nor a remote mirror.
use crate::state::AppState;
use rupu_orchestrator::RunRecord;
use rupu_runtime::{RunEnvelope, WorkerKind, WorkerRecord};
use std::{collections::HashMap, io::Read, path::{Path, PathBuf}};

pub(super) const FEATURE: &str = "workflow.session_history.local_worker.v1";
const MAX_PROOF_BYTES: usize = 32 * 1024 * 1024;
const MAX_WORKERS: usize = 512;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Ownership {
    Stored,
    Cli,
    Autoflow,
    Excluded,
    Unavailable,
}
impl Ownership {
    pub fn origin(self) -> Option<&'static str> {
        match self {
            Self::Stored => Some("local_recorded"),
            Self::Cli => Some("local_cli"),
            Self::Autoflow => Some("local_autoflow"),
            Self::Excluded | Self::Unavailable => None,
        }
    }
}

pub(super) struct Classifier {
    hostname: Option<String>,
    roots: HashMap<PathBuf, Option<PathBuf>>,
    workers: HashMap<String, Ownership>,
    bytes: usize,
    attempts: usize,
    pub excluded: usize,
    pub unavailable: usize,
}
impl Classifier {
    pub fn new() -> Self {
        Self::with_hostname(whoami::fallible::hostname().ok())
    }
    fn with_hostname(hostname: Option<String>) -> Self {
        Self {
            hostname: hostname.filter(|s| !s.is_empty() && s != "unknown-host"),
            workers: HashMap::new(),
            roots: HashMap::new(),
            bytes: 0,
            attempts: 0,
            excluded: 0,
            unavailable: 0,
        }
    }
    // Configured owner roots may contain an OS/home alias. Resolve only these
    // trusted roots once per observation, never a worker/run proof descendant.
    fn root(&mut self, configured: &Path) -> Option<PathBuf> {
        self.roots.entry(configured.to_path_buf()).or_insert_with(|| {
            std::fs::canonicalize(configured).ok().filter(|p| p.is_dir())
        }).clone()
    }
    fn read(&mut self, path: &Path, cap: usize) -> Option<Vec<u8>> {
        let cap = cap.min(MAX_PROOF_BYTES.saturating_sub(self.bytes));
        if cap == 0 {
            return None;
        }
        let file = super::fs_open::open_anchored_regular_file(path).ok()?;
        let before = file.metadata().ok()?;
        if before.len() > cap as u64 {
            return None;
        }
        let mut data = Vec::new();
        // Charge even failed/oversized reads against the request-wide budget.
        let result = (&file).take(cap as u64 + 1).read_to_end(&mut data);
        self.bytes = self.bytes.saturating_add(data.len());
        if result.is_err() || data.len() > cap {
            return None;
        }
        let after = file.metadata().ok()?;
        if before.len() != after.len() || before.modified().ok()? != after.modified().ok()? {
            return None;
        }
        Some(data)
    }
    fn worker(&mut self, s: &AppState, id: &str) -> Ownership {
        if id.is_empty()
            || id.len() > 128
            || !id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        {
            return Ownership::Unavailable;
        }
        // Recheck collisions even when the local worker proof is cached.
        if s.node_registry.get(id).is_some() {
            return Ownership::Excluded;
        }
        match s.hosts.has_persisted_identity(id) {
            Ok(true) => return Ownership::Excluded,
            Err(_) => return Ownership::Unavailable,
            Ok(false) => (),
        }
        // The readonly acceptance registry may intentionally omit configured
        // remote hosts. Their durable names still prohibit local reassignment.
        match std::fs::symlink_metadata(s.global_dir.join("hosts").join(format!("{id}.toml"))) {
            Ok(_) => return Ownership::Excluded,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(_) => return Ownership::Unavailable,
        }
        if let Some(value) = self.workers.get(id) {
            return *value;
        }
        if self.workers.len() >= MAX_WORKERS {
            return Ownership::Unavailable;
        }
        let Some(global) = self.root(&s.global_dir) else { return Ownership::Unavailable; };
        let value = self
            .read(
                &global
                    .join("autoflows/workers")
                    .join(format!("{id}.toml")),
                64 * 1024,
            )
            .and_then(|data| {
                std::str::from_utf8(&data)
                    .ok()
                    .and_then(|text| toml::from_str::<WorkerRecord>(text).ok())
            })
            .filter(|record| record.version == WorkerRecord::VERSION && record.worker_id == id)
            .filter(|record| self.hostname.as_deref() == Some(record.host.as_str()))
            .map(|record| match record.kind {
                WorkerKind::Cli => Ownership::Cli,
                WorkerKind::AutoflowServe => Ownership::Autoflow,
            })
            .unwrap_or(Ownership::Unavailable);
        self.workers.insert(id.into(), value);
        value
    }
    pub fn classify(&mut self, s: &AppState, run: &RunRecord, archived: bool) -> Ownership {
        let Some(id) = run.worker_id.as_deref() else {
            return Ownership::Stored;
        };
        if self.attempts >= 4096 {
            self.unavailable += 1;
            return Ownership::Unavailable;
        }
        self.attempts += 1;
        let mut result = self.worker(s, id);
        if matches!(result, Ownership::Cli | Ownership::Autoflow) {
            let root = if archived {
                s.run_store.archive_root()
            } else {
                s.run_store.root.clone()
            };
            let proof = if super::runs::validate_id(&run.id).is_ok() {
                self.root(&root).and_then(|root| self.read(&root.join(&run.id).join("run_envelope.json"), 256 * 1024))
                    .and_then(|data| serde_json::from_slice::<RunEnvelope>(&data).ok())
            } else {
                None
            };
            if !proof.is_some_and(|e| {
                e.version == RunEnvelope::VERSION
                    && e.run_id == run.id
                    && e.workflow.name == run.workflow_name
                    && e.repo.as_ref().is_some_and(|repo| {
                        repo.workspace_id == run.workspace_id
                            && repo.workspace_path == run.workspace_path
                    })
                    && e.worker
                        .as_ref()
                        .and_then(|w| w.assigned_worker_id.as_deref())
                        == Some(id)
                    && e.execution.backend.as_deref() == Some("local_worktree")
                    && run.backend_id.as_deref() == Some("local_worktree")
            }) {
                result = Ownership::Unavailable;
            }
        }
        match result {
            Ownership::Excluded => self.excluded += 1,
            Ownership::Unavailable => self.unavailable += 1,
            _ => (),
        }
        result
    }
}
