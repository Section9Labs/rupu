//! Session records: `session.json`'s schema and its versioned parse, where
//! sessions live (`<global>/sessions`, `<global>/sessions-archive`), and the
//! archived-session prune `rupu session prune`, `rupu cleanup` and the
//! control plane's `POST /api/maintenance/prune` share. `rupu session` owns
//! the lifecycle (start, turns, compaction, archive); everything that reads
//! or deletes a record reads it through here.

use crate::retention::{parse_retention_duration, PruneError};
use crate::standalone_run_metadata::{archived_transcripts_dir, metadata_path_for_run};
use anyhow::Context;
use chrono::{DateTime, Utc};
use rupu_providers::model_tier::{ContextWindow, ThinkingLevel};
use rupu_providers::types::{
    ContextManagement, Message, OutputFormat as ProviderOutputFormat, Speed, ThinkingDisplay,
};
use rupu_providers::AuthMode;
use rupu_transcript::RunStatus;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

/// An anyhow failure (its whole chain) as a prune failure.
fn failed(e: anyhow::Error) -> PruneError {
    PruneError::Failed(format!("{e:#}"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    Idle,
    Running,
    Failed,
    Stopped,
}

impl SessionStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Running => "running",
            Self::Failed => "failed",
            Self::Stopped => "stopped",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionScope {
    Active,
    Archived,
}

impl SessionScope {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Archived => "archived",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionRunRecord {
    pub run_id: String,
    pub prompt: String,
    pub transcript_path: PathBuf,
    pub started_at: DateTime<Utc>,
    #[serde(default)]
    pub completed_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub status: Option<RunStatus>,
    #[serde(default)]
    pub total_tokens_in: u64,
    #[serde(default)]
    pub total_tokens_out: u64,
    #[serde(default)]
    pub total_tokens_cached: u64,
    #[serde(default)]
    pub duration_ms: u64,
    #[serde(default)]
    pub pid: Option<u32>,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionRecord {
    pub version: u32,
    pub session_id: String,
    pub agent_name: String,
    /// `crew/role` — one identity across every turn (spec §3). `None` for
    /// sessions created before codenames; readers use `derive_legacy`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codename: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    pub provider_name: String,
    #[serde(default)]
    pub auth_mode: Option<AuthMode>,
    pub model: String,
    pub agent_system_prompt: String,
    #[serde(default)]
    pub agent_tools: Option<Vec<String>>,
    /// The agent's `messaging:` key (spec F1d D4), frozen with its tools.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub messaging: Option<bool>,
    pub max_turns: u32,
    pub permission_mode: String,
    pub no_stream: bool,
    #[serde(default)]
    pub anthropic_oauth_prefix: Option<bool>,
    #[serde(default)]
    pub anthropic_prompt_cache: Option<bool>,
    #[serde(default)]
    pub effort: Option<ThinkingLevel>,
    #[serde(default)]
    pub thinking_display: Option<ThinkingDisplay>,
    #[serde(default)]
    pub context_window: Option<ContextWindow>,
    #[serde(default)]
    pub output_format: Option<ProviderOutputFormat>,
    #[serde(default)]
    pub output_schema: Option<serde_json::Value>,
    #[serde(default)]
    pub anthropic_task_budget: Option<u32>,
    #[serde(default)]
    pub anthropic_context_management: Option<ContextManagement>,
    #[serde(default)]
    pub anthropic_speed: Option<Speed>,
    #[serde(default)]
    pub dispatchable_agents: Option<Vec<String>>,
    #[serde(default)]
    pub dispatchable_workflows: Option<Vec<String>>,
    /// The session agent's `findingsProfile` frontmatter, snapshotted at
    /// session creation like the other agent-spec fields above. Absent ⇒
    /// `full`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub findings_profile: Option<rupu_coverage::FindingProfile>,
    /// The engagement profile ids `session start --engagement-profile`
    /// selected; every turn re-resolves them against the workspace. Empty =
    /// the `code` path (and every session that predates the field).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub engagement_profiles: Vec<String>,
    pub workspace_id: String,
    pub workspace_path: PathBuf,
    #[serde(default)]
    pub project_root: Option<PathBuf>,
    /// The directory `session start` ran in — the customer lookup's
    /// `run_dir` for every turn, so a session on a cloned target keeps the
    /// customer of the directory it was started from. Absent on sessions
    /// that predate it: those look the customer up from `workspace_path`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launch_dir: Option<PathBuf>,
    /// The customer the session's directory is assigned to (`rupu
    /// customer`), as of its latest turn: each turn re-resolves it, so
    /// a reassignment mid-session shows here while every transcript keeps
    /// the customer it ran under. Tri-state (`rupu_transcript::recorded`):
    /// `None` = the key is absent (a session that predates customers),
    /// `Some(None)` = recorded "no customer" (written as `null`).
    #[serde(
        default,
        deserialize_with = "rupu_transcript::recorded::deserialize",
        skip_serializing_if = "Option::is_none"
    )]
    pub customer: rupu_transcript::RecordedField,
    pub transcripts_dir: PathBuf,
    #[serde(default)]
    pub repo_ref: Option<String>,
    #[serde(default)]
    pub issue_ref: Option<String>,
    #[serde(default)]
    pub target: Option<String>,
    #[serde(default)]
    pub workspace_strategy: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub status: SessionStatus,
    #[serde(default)]
    pub active_run_id: Option<String>,
    #[serde(default)]
    pub active_transcript_path: Option<PathBuf>,
    #[serde(default)]
    pub active_pid: Option<u32>,
    #[serde(default)]
    pub worker_pid: Option<u32>,
    /// Unix mtime (secs) of the `rupu` binary that spawned the current worker.
    /// On the next turn we compare it to the running binary's mtime; if the
    /// binary is newer the worker is stale (running old code) and is restarted
    /// so a freshly-installed build takes effect without a manual stop.
    #[serde(default)]
    pub worker_binary_mtime: Option<i64>,
    #[serde(default)]
    pub last_run_id: Option<String>,
    #[serde(default)]
    pub last_transcript_path: Option<PathBuf>,
    #[serde(default)]
    pub last_error: Option<String>,
    #[serde(default)]
    pub total_turns: u32,
    #[serde(default)]
    pub total_tokens_in: u64,
    #[serde(default)]
    pub total_tokens_out: u64,
    #[serde(default)]
    pub total_tokens_cached: u64,
    #[serde(default)]
    pub message_history: Vec<Message>,
    #[serde(default)]
    pub runs: Vec<SessionRunRecord>,
    /// Coverage concerns block resolved from the agent's frontmatter at
    /// session start. When `Some`, each turn runs with the coverage
    /// harness active (target_id derived from `(workspace, session_id)`).
    #[serde(default)]
    pub concerns: Option<rupu_coverage::ConcernsBlock>,
    /// Optional agent pin for the per-request output-token cap, captured from
    /// `spec.max_tokens` at session start. When set it overrides the cap
    /// discovered from the provider's model list; when neither is known,
    /// Anthropic gets 8192 and other providers get no cap.
    #[serde(default)]
    pub max_tokens: Option<u32>,
    /// Agent pin for the input limit (`contextWindowTokens`), captured from
    /// the spec at session start. Overrides the discovered limit (`resolve`
    /// applies pins first); compaction and the gauge read
    /// `effective_context_window()`.
    #[serde(default)]
    pub context_window_tokens: Option<u32>,
    /// Agent pin for the compaction percentage (`compactAtPercent`), captured
    /// from the spec at session start; read through
    /// `effective_compact_at_percent()`.
    #[serde(default)]
    pub compact_at_percent: Option<u8>,
    /// Limits resolved on the session's first turn (spec 2026-09-30 §6.5),
    /// including any limit learned from an overflow error. Reused on every
    /// later turn; never refetched mid-session.
    #[serde(default)]
    pub model_limits: Option<rupu_providers::model_limits::ModelLimits>,
    /// Path of a transcript whose replay-reconstruction equals
    /// `message_history` byte-exact — the ONLY legitimate source for
    /// `seed_source` when starting the next turn (spec §3 seed dedup
    /// rule). Set ONLY at the single point where `message_history` is
    /// replaced with a successful turn's `final_messages`; cleared to
    /// `None` at every other mutation of `message_history` (compaction,
    /// in both the offline CLI and worker pseudo-turn paths) so a turn
    /// whose transcript does NOT reconstruct to the current history can
    /// never be referenced. Distinct from `last_transcript_path`, which
    /// is a UX pointer updated unconditionally (including on failed/
    /// stopped/crashed turns) and must never be used to derive a seed.
    #[serde(default)]
    pub history_source_transcript: Option<PathBuf>,
    /// The session's skills, frozen by `session start` (spec 2026-10-12
    /// §5.1): exactly the skills every turn is given, whatever is installed
    /// later. Empty for a session with none, and for every session that
    /// predates skills.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skill_index: Vec<rupu_skills::FrozenSkill>,
    /// The skills section `session start` rendered: sent as it is on every
    /// turn, so each turn's system prompt is the first's. `None` when the
    /// session has no skills section.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skill_prompt: Option<String>,
    /// The skills the conversation has loaded, most recently loaded last,
    /// as the latest turn ended: the next turn starts with them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub loaded_skills: Vec<rupu_agent::LoadedSkill>,
    /// Messages in `message_history` no model call saw (the latest turn
    /// failed first): the next turn's first call writes their receipts.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unseen_messages: Vec<String>,
}

impl SessionRecord {
    /// The newest record version this rupu reads. A record is written at
    /// [`Self::written_version`].
    pub const VERSION: u32 = 3;

    /// The lowest version a rupu must understand to keep the record whole on
    /// its next write: 3 for an agent's `messaging: false`, 2 for skills,
    /// else 1, so older rupus keep reading what they can't damage.
    pub fn written_version(&self) -> u32 {
        let skills = !self.skill_index.is_empty()
            || self.skill_prompt.is_some()
            || !self.loaded_skills.is_empty();
        if self.messaging == Some(false) {
            3
        } else if skills {
            2
        } else {
            1
        }
    }

    /// The input limit compaction and the gauge use: the stored resolved value
    /// (which already reflects the agent pin — `resolve` applies pins first —
    /// and any limit learned from an overflow error), else the agent pin.
    pub fn effective_context_window(&self) -> Option<u32> {
        self.model_limits
            .as_ref()
            .and_then(|l| l.input.tokens)
            // A 0 pin is no limit (`resolve` ignores it too).
            .or(self.context_window_tokens.filter(|n| *n > 0))
    }

    /// The compact-at percentage compaction uses: the stored value when the
    /// stored limits actually resolved something (it already reflects the
    /// pin — `resolve` applies pins first), else the agent pin, else the
    /// default. Unresolved stored limits (`ModelLimits::unknown()`, a
    /// transient first-turn failure) carry only the default percentage, which
    /// must not outrank a pin.
    pub fn effective_compact_at_percent(&self) -> u8 {
        self.model_limits
            .as_ref()
            .filter(|l| !l.is_unresolved())
            .map(|l| l.compact_at_percent)
            .or(self.compact_at_percent)
            .unwrap_or(rupu_providers::model_limits::DEFAULT_COMPACT_AT_PERCENT)
    }

    /// The input-token threshold manual compaction keeps the history under:
    /// the same `ModelLimits::compact_threshold` a run compacts at (spec
    /// 2026-09-30 §6.4, headroom rule included), built from the session's
    /// effective limits — the stored resolved limits, else the agent pins —
    /// with `window_override` (`session compact --window`) replacing the input
    /// side. `None` when no input limit is known or pinned.
    pub fn compaction_threshold(&self, window_override: Option<u32>) -> Option<u64> {
        use rupu_providers::model_limits::{Limit, LimitSource, ModelLimits};
        let input = window_override.or(self.effective_context_window())?;
        let mut limits = match &self.model_limits {
            Some(l) if !l.is_unresolved() => l.clone(),
            stored => {
                let mut l = ModelLimits::from_pins(None, self.max_tokens.filter(|n| *n > 0), None);
                if let Some(stored) = stored {
                    l.output_shares_context = stored.output_shares_context;
                }
                l
            }
        };
        if limits.input.tokens != Some(input) {
            limits.input = Limit::new(input, LimitSource::Agent);
        }
        limits.compact_at_percent = self.effective_compact_at_percent();
        limits.compact_threshold()
    }
}

/// A session record as it sits on disk.
pub enum StoredSession {
    /// One this rupu reads and may write back.
    Current(SessionRecord),
    /// One a newer rupu wrote: it may be shown, never written back, and
    /// what it owns may be named in fields this rupu doesn't know.
    Newer { version: u32, record: SessionRecord },
}

impl StoredSession {
    /// The warning a listing prints for a record it can only show.
    pub fn newer_warning(&self) -> Option<String> {
        match self {
            StoredSession::Current(_) => None,
            StoredSession::Newer { version, record } => Some(format!(
                "warning: session {} was written by a newer rupu (record version {version}); \
                 upgrade rupu to manage it",
                record.session_id
            )),
        }
    }

    pub fn record(&self) -> &SessionRecord {
        match self {
            StoredSession::Current(record) => record,
            StoredSession::Newer { record, .. } => record,
        }
    }
}

/// The one parse of a session record: its version decides whether this
/// rupu may write it back.
pub fn parse_stored_session(bytes: &[u8]) -> anyhow::Result<StoredSession> {
    #[derive(Deserialize)]
    struct Versioned {
        version: u32,
    }
    let Versioned { version } = serde_json::from_slice(bytes)?;
    let record = serde_json::from_slice(bytes)?;
    Ok(if version <= SessionRecord::VERSION {
        StoredSession::Current(record)
    } else {
        StoredSession::Newer { version, record }
    })
}

/// A session record this rupu understands: one a newer rupu wrote may hold
/// fields this one would drop when it writes the record back.
pub fn parse_session_record(bytes: &[u8]) -> anyhow::Result<SessionRecord> {
    match parse_stored_session(bytes)? {
        StoredSession::Current(record) => Ok(record),
        StoredSession::Newer { .. } => {
            anyhow::bail!("this session was written by a newer rupu; upgrade rupu to continue it")
        }
    }
}

/// `<global>/sessions`: the active sessions, one directory each.
pub fn sessions_dir(global: &Path) -> PathBuf {
    global.join("sessions")
}

/// `<global>/sessions-archive`: the archived sessions.
pub fn archived_sessions_dir(global: &Path) -> PathBuf {
    global.join("sessions-archive")
}

/// The directory of `scope`'s sessions.
pub fn scope_dir(global: &Path, scope: SessionScope) -> PathBuf {
    match scope {
        SessionScope::Active => sessions_dir(global),
        SessionScope::Archived => archived_sessions_dir(global),
    }
}

/// `session_id`'s directory in `scope`.
pub fn session_dir(global: &Path, scope: SessionScope, session_id: &str) -> PathBuf {
    scope_dir(global, scope).join(session_id)
}

/// `session_id`'s `session.json` in `scope`.
pub fn session_record_path(global: &Path, scope: SessionScope, session_id: &str) -> PathBuf {
    session_dir(global, scope, session_id).join("session.json")
}

/// Every session record in `scope`, as stored ([`parse_stored_session`]).
pub fn load_sessions_in_scope(
    global: &Path,
    scope: SessionScope,
) -> anyhow::Result<Vec<StoredSession>> {
    let dir = scope_dir(global, scope);
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for entry in fs::read_dir(&dir)? {
        let entry = entry?;
        let path = entry.path().join("session.json");
        if !path.is_file() {
            continue;
        }
        let bytes = fs::read(&path)?;
        out.push(
            parse_stored_session(&bytes)
                .with_context(|| format!("read session record {}", path.display()))?,
        );
    }
    Ok(out)
}

/// Delete the transcripts (and their metadata sidecars) of every run the
/// session owns, active or archived.
pub fn delete_session_owned_artifacts(session: &SessionRecord) -> anyhow::Result<()> {
    let active_root = session.transcripts_dir.clone();
    let archived_root = archived_transcripts_dir(&active_root);
    let mut seen = HashSet::new();
    for run in &session.runs {
        if !seen.insert(run.run_id.clone()) {
            continue;
        }
        let active_path = active_root.join(format!("{}.jsonl", run.run_id));
        let archived_path = archived_root.join(format!("{}.jsonl", run.run_id));
        let active_meta = metadata_path_for_run(&active_root, &run.run_id);
        let archived_meta = metadata_path_for_run(&archived_root, &run.run_id);
        remove_file_if_exists(&run.transcript_path)?;
        remove_file_if_exists(&active_path)?;
        remove_file_if_exists(&archived_path)?;
        remove_file_if_exists(&active_meta)?;
        remove_file_if_exists(&archived_meta)?;
    }
    Ok(())
}

/// Remove `path` when it exists.
pub fn remove_file_if_exists(path: &Path) -> anyhow::Result<()> {
    if path.exists() {
        fs::remove_file(path).with_context(|| format!("remove {}", path.display()))?;
    }
    Ok(())
}

/// The cutoff of a session prune: now minus `older_than`, else minus the
/// global `[storage].archived_session_retention` (default `30d`). A bad
/// `older_than` is the caller's ([`PruneError::InvalidRetention`]); a bad
/// `[storage]` value is the configuration's ([`PruneError::Config`]); a
/// global config that can't be read is a [`PruneError::Failed`] carrying
/// its whole chain.
pub fn session_prune_cutoff(
    older_than: Option<&str>,
    global: &Path,
) -> Result<DateTime<Utc>, PruneError> {
    let retention = match older_than {
        Some(value) => parse_retention_duration(value)?,
        None => {
            let path = global.join("config.toml");
            let cfg = rupu_config::layer_files_locked(rupu_config::LayerPaths::global_only(&path))
                .map_err(|e| failed(anyhow::Error::new(e)))?;
            let value = cfg
                .storage
                .archived_session_retention
                .unwrap_or_else(|| "30d".to_string());
            parse_retention_duration(&value).map_err(|e| PruneError::Config(e.to_string()))?
        }
    };
    Ok(Utc::now() - retention)
}

/// One archived session a prune removed, or would remove.
#[derive(Debug, Clone)]
pub struct PrunedSession {
    pub session_id: String,
    pub scope: String,
    pub status: String,
    pub updated_at: String,
    pub action: String,
}

/// Delete (or, with `dry_run`, only list) every archived session last
/// updated before the cutoff ([`session_prune_cutoff`]), with the
/// transcripts it owns. A record a newer rupu wrote is never pruned: its
/// [`StoredSession::newer_warning`] goes to `on_warning` and it is kept.
/// Rows are in directory order; [`prune_report`] sorts them.
pub fn prune_archived_sessions(
    global: &Path,
    older_than: Option<&str>,
    dry_run: bool,
    on_warning: &mut dyn FnMut(&str),
) -> Result<Vec<PrunedSession>, PruneError> {
    let cutoff = session_prune_cutoff(older_than, global)?;
    let mut rows = Vec::new();
    for stored in load_sessions_in_scope(global, SessionScope::Archived).map_err(failed)? {
        if stored.record().updated_at > cutoff {
            continue;
        }
        let session = match stored {
            StoredSession::Current(session) => session,
            newer => {
                if let Some(warning) = newer.newer_warning() {
                    on_warning(&warning);
                }
                continue;
            }
        };
        rows.push(PrunedSession {
            session_id: session.session_id.clone(),
            scope: SessionScope::Archived.as_str().to_string(),
            status: session.status.as_str().to_string(),
            updated_at: session.updated_at.to_rfc3339(),
            action: if dry_run {
                "would_delete".into()
            } else {
                "deleted".into()
            },
        });
        if !dry_run {
            delete_session_owned_artifacts(&session).map_err(failed)?;
            let dir = session_dir(global, SessionScope::Archived, &session.session_id);
            if dir.exists() {
                fs::remove_dir_all(&dir)
                    .with_context(|| format!("remove session dir {}", dir.display()))
                    .map_err(failed)?;
            }
        }
    }
    Ok(rows)
}

/// A row of `rupu session prune`'s JSON report.
#[derive(Debug, Clone, Serialize)]
pub struct SessionPruneRow {
    pub session_id: String,
    pub scope: String,
    pub status: String,
    pub updated_at: String,
    pub action: String,
}

/// `rupu session prune`'s JSON report.
#[derive(Debug, Clone, Serialize)]
pub struct SessionPruneReport {
    pub kind: &'static str,
    pub version: u8,
    pub rows: Vec<SessionPruneRow>,
}

/// `rupu session prune`'s report, sorted by session id: what the command
/// prints and what the control plane's `POST /api/maintenance/prune {kind:
/// sessions}` answers.
pub fn prune_report(
    global: &Path,
    older_than: Option<&str>,
    dry_run: bool,
    on_warning: &mut dyn FnMut(&str),
) -> Result<SessionPruneReport, PruneError> {
    let mut pruned = prune_archived_sessions(global, older_than, dry_run, on_warning)?;
    pruned.sort_by(|a, b| a.session_id.cmp(&b.session_id));
    Ok(SessionPruneReport {
        kind: "session_prune",
        version: 1,
        rows: pruned
            .into_iter()
            .map(|row| SessionPruneRow {
                session_id: row.session_id,
                scope: row.scope,
                status: row.status,
                updated_at: row.updated_at,
                action: row.action,
            })
            .collect(),
    })
}
