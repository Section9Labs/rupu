//! Per-run usage ledger (spec 2026-09-29 §3): one JSON row per LLM call made
//! anywhere inside a workflow run, appended to `<runs>/<id>/usage.jsonl`.
//! Rows carry a ULID `id` so any consumer that sees a line twice (mirror
//! replays, tailer races) counts it once. Writing never fails a run.

use crate::runs::RunStore;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

pub const LEDGER_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LedgerKind {
    Turn,
    Compaction,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LedgerRow {
    pub v: u32,
    pub id: String,
    pub at: DateTime<Utc>,
    pub kind: LedgerKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit_index: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit_key: Option<String>,
    pub agent_run_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_agent_run_id: Option<String>,
    pub transcript: PathBuf,
    pub agent: String,
    pub provider: String,
    pub model: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// Cache reads — a subset of `input_tokens`.
    #[serde(default)]
    pub cached_tokens: u64,
    /// Cache writes — a subset of `input_tokens`, like `cached_tokens`.
    /// Absent on rows written before it existed (reads as `0`).
    #[serde(default)]
    pub cache_write_tokens: u64,
}

/// Where in the workflow an agent run sits. `step_id: None` = a dispatched
/// sub-agent (attributed to its ancestor's step by the fold).
#[derive(Debug, Clone, Default)]
pub struct LedgerTag {
    pub step_id: Option<String>,
    pub unit_index: Option<usize>,
    pub unit_key: Option<String>,
}

/// Running totals for one unit, filled by its hook — the honest source for
/// `UnitCompleted.tokens_in/out` (spec §3.4), on success AND failure.
#[derive(Debug, Default)]
pub struct UnitTokenCounters {
    pub input: AtomicU64,
    pub output: AtomicU64,
}

/// Cloneable append handle. The file opens lazily on first append (so a
/// ledger for a run that never calls a model creates no file) with
/// `O_APPEND | O_CREAT`; each row is ONE `write_all` of `json + "\n"` under
/// the lock, so rows from several handles/processes never interleave.
#[derive(Clone)]
pub struct UsageLedger {
    path: PathBuf,
    file: Arc<Mutex<Option<std::fs::File>>>,
}

impl std::fmt::Debug for UsageLedger {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UsageLedger")
            .field("path", &self.path)
            .finish()
    }
}

impl UsageLedger {
    pub fn open(path: PathBuf) -> Self {
        Self {
            path,
            file: Arc::new(Mutex::new(None)),
        }
    }

    pub fn for_run(store: &RunStore, run_id: &str) -> Self {
        Self::open(store.usage_ledger_path(run_id))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Append one row. Errors are logged and swallowed — usage accounting
    /// must never fail a run.
    pub fn append(&self, row: &LedgerRow) {
        let mut line = match serde_json::to_vec(row) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(error = %e, "usage ledger: serialize failed");
                return;
            }
        };
        line.push(b'\n');
        let mut guard = match self.file.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        if guard.is_none() {
            match std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.path)
            {
                Ok(f) => *guard = Some(f),
                Err(e) => {
                    tracing::warn!(path = %self.path.display(), error = %e, "usage ledger: open failed");
                    return;
                }
            }
        }
        if let Some(f) = guard.as_mut() {
            if let Err(e) = f.write_all(&line).and_then(|_| f.flush()) {
                tracing::warn!(path = %self.path.display(), error = %e, "usage ledger: write failed");
            }
        }
    }

    /// Build the agent-loop `on_usage` hook for one agent run.
    pub fn hook(
        &self,
        tag: LedgerTag,
        agent_run_id: String,
        parent_agent_run_id: Option<String>,
        transcript: PathBuf,
        agent: String,
        counters: Option<Arc<UnitTokenCounters>>,
    ) -> rupu_agent::OnUsageCallback {
        let ledger = self.clone();
        Arc::new(move |u: &rupu_agent::UsageTurn| {
            if let Some(c) = &counters {
                c.input.fetch_add(u.input_tokens, Ordering::Relaxed);
                c.output.fetch_add(u.output_tokens, Ordering::Relaxed);
            }
            ledger.append(&LedgerRow {
                v: LEDGER_VERSION,
                id: ulid::Ulid::new().to_string(),
                at: Utc::now(),
                kind: match u.kind {
                    rupu_agent::UsageKind::Turn => LedgerKind::Turn,
                    rupu_agent::UsageKind::Compaction => LedgerKind::Compaction,
                },
                step_id: tag.step_id.clone(),
                unit_index: tag.unit_index,
                unit_key: tag.unit_key.clone(),
                agent_run_id: agent_run_id.clone(),
                parent_agent_run_id: parent_agent_run_id.clone(),
                transcript: transcript.clone(),
                agent: agent.clone(),
                provider: u.provider.clone(),
                model: u.model.clone(),
                input_tokens: u.input_tokens,
                output_tokens: u.output_tokens,
                cached_tokens: u.cached_tokens,
                cache_write_tokens: u.cache_write_tokens,
            });
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn turn(input: u64) -> rupu_agent::UsageTurn {
        rupu_agent::UsageTurn {
            kind: rupu_agent::UsageKind::Turn,
            provider: "anthropic".into(),
            model: "claude-x".into(),
            input_tokens: input,
            output_tokens: 3,
            cached_tokens: 1,
            cache_write_tokens: 0,
        }
    }

    #[test]
    fn hook_appends_one_attributed_row_per_call() {
        let dir = tempfile::tempdir().unwrap();
        let led = UsageLedger::open(dir.path().join("usage.jsonl"));
        let counters = Arc::new(UnitTokenCounters::default());
        let hook = led.hook(
            LedgerTag {
                step_id: Some("assess".into()),
                unit_index: Some(2),
                unit_key: Some("gw".into()),
            },
            "run_A".into(),
            None,
            dir.path().join("run_A.jsonl"),
            "reviewer".into(),
            Some(counters.clone()),
        );
        hook(&turn(100));
        hook(&turn(50));
        let body = std::fs::read_to_string(led.path()).unwrap();
        let rows: Vec<LedgerRow> = body
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(rows.len(), 2);
        assert_ne!(rows[0].id, rows[1].id);
        assert_eq!(rows[0].v, LEDGER_VERSION);
        assert_eq!(rows[0].step_id.as_deref(), Some("assess"));
        assert_eq!(rows[0].unit_index, Some(2));
        assert_eq!(rows[0].agent_run_id, "run_A");
        assert_eq!(rows[1].input_tokens, 50);
        assert_eq!(rows[0].kind, LedgerKind::Turn);
        assert_eq!(counters.input.load(Ordering::Relaxed), 150);
        assert_eq!(counters.output.load(Ordering::Relaxed), 6);
    }

    #[test]
    fn hook_copies_cache_write_tokens_into_the_row() {
        let dir = tempfile::tempdir().unwrap();
        let led = UsageLedger::open(dir.path().join("usage.jsonl"));
        let hook = led.hook(
            LedgerTag::default(),
            "run_W".into(),
            None,
            dir.path().join("run_W.jsonl"),
            "writer".into(),
            None,
        );
        hook(&rupu_agent::UsageTurn {
            cache_write_tokens: 30,
            ..turn(100)
        });
        let body = std::fs::read_to_string(led.path()).unwrap();
        let row: LedgerRow = serde_json::from_str(body.lines().next().unwrap()).unwrap();
        assert_eq!(row.cache_write_tokens, 30);
        assert_eq!(row.cached_tokens, 1);
    }

    #[test]
    fn old_ledger_row_without_cache_write_tokens_reads_as_zero() {
        let old = r#"{"v":1,"id":"01X","at":"2026-09-29T00:00:00Z","kind":"turn","agent_run_id":"r","transcript":"/t/r.jsonl","agent":"a","provider":"p","model":"m","input_tokens":1,"output_tokens":2,"cached_tokens":0}"#;
        let row: LedgerRow = serde_json::from_str(old).unwrap();
        assert_eq!(row.cache_write_tokens, 0);
    }

    #[test]
    fn concurrent_appends_from_two_handles_never_interleave() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("usage.jsonl");
        let a = UsageLedger::open(path.clone());
        let b = UsageLedger::open(path.clone());
        let mut hs = Vec::new();
        for led in [a, b] {
            hs.push(std::thread::spawn(move || {
                let h = led.hook(
                    LedgerTag::default(),
                    "run_X".into(),
                    None,
                    std::path::PathBuf::from("/t/run_X.jsonl"),
                    "a".into(),
                    None,
                );
                for i in 0..500 {
                    h(&turn(i));
                }
            }));
        }
        for h in hs {
            h.join().unwrap();
        }
        let body = std::fs::read_to_string(&path).unwrap();
        assert_eq!(body.lines().count(), 1000);
        for l in body.lines() {
            serde_json::from_str::<LedgerRow>(l).expect("every line is a whole row");
        }
    }

    #[test]
    fn unwritable_ledger_never_panics() {
        let led = UsageLedger::open(std::path::PathBuf::from("/nonexistent-dir-xyz/usage.jsonl"));
        let h = led.hook(
            LedgerTag::default(),
            "r".into(),
            None,
            "/t/r.jsonl".into(),
            "a".into(),
            None,
        );
        h(&turn(1)); // must not panic
    }

    #[test]
    fn row_json_shape_is_stable() {
        let row = LedgerRow {
            v: 1,
            id: "01X".into(),
            at: chrono::DateTime::parse_from_rfc3339("2026-09-29T00:00:00Z")
                .unwrap()
                .with_timezone(&chrono::Utc),
            kind: LedgerKind::Compaction,
            step_id: None,
            unit_index: None,
            unit_key: None,
            agent_run_id: "sub_1".into(),
            parent_agent_run_id: Some("run_P".into()),
            transcript: "/t/sub_1/transcript.jsonl".into(),
            agent: "a".into(),
            provider: "p".into(),
            model: "m".into(),
            input_tokens: 1,
            output_tokens: 2,
            cached_tokens: 0,
            cache_write_tokens: 0,
        };
        let v = serde_json::to_value(&row).unwrap();
        assert_eq!(v["kind"], "compaction");
        assert_eq!(v["parent_agent_run_id"], "run_P");
        assert!(v.get("step_id").is_none(), "None fields are omitted");
    }
}
