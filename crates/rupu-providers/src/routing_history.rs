//! Routing history — tracks per-model per-task-type outcomes.
//!
//! Persisted to `cortex/routing_history.json`. Loaded at boot,
//! updated after each call. Old entries decay on load.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::provider_id::ProviderId;
use crate::task_classifier::TaskType;

/// Flush to disk every N updates.
const FLUSH_INTERVAL: u32 = 10;
/// Entries older than this get decayed on load.
const DECAY_DAYS: i64 = 7;
/// Maximum entries in the history. Oldest entries evicted when exceeded.
const MAX_ENTRIES: usize = 10_000;
/// Maximum file size to load (10 MB). Files larger than this are rejected.
const MAX_FILE_SIZE: u64 = 10 * 1024 * 1024;

/// A single history entry for a model+task_type combination.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub successes: u32,
    pub failures: u32,
    pub total_latency_ms: u64,
    pub last_updated: DateTime<Utc>,
}

impl HistoryEntry {
    /// Success rate: successes / total. Returns 0.5 for empty entries.
    pub fn success_rate(&self) -> f64 {
        let total = self.successes + self.failures;
        if total == 0 {
            0.5
        } else {
            self.successes as f64 / total as f64
        }
    }

    /// Average latency in milliseconds. Returns 0 for empty entries.
    pub fn avg_latency_ms(&self) -> u64 {
        let total = self.successes + self.failures;
        if total == 0 {
            0
        } else {
            self.total_latency_ms / total as u64
        }
    }
}

/// Tracks per-model per-task-type outcomes.
pub struct RoutingHistory {
    entries: HashMap<String, HistoryEntry>,
    pending_writes: u32,
    last_flush: Instant,
    persist_path: PathBuf,
}

impl RoutingHistory {
    /// Build a history key from components.
    pub fn key(provider: ProviderId, model: &str, task_type: TaskType) -> String {
        format!("{provider}:{model}:{task_type}")
    }

    /// Load from disk. If file missing or corrupt, start empty.
    pub fn load(path: &Path) -> Self {
        let entries = if path.exists() {
            // Reject oversized files to prevent OOM
            if let Ok(meta) = std::fs::metadata(path) {
                if meta.len() > MAX_FILE_SIZE {
                    warn!(
                        size = meta.len(),
                        max = MAX_FILE_SIZE,
                        "routing history file too large, starting fresh"
                    );
                    return Self {
                        entries: HashMap::new(),
                        pending_writes: 0,
                        last_flush: Instant::now(),
                        persist_path: path.to_path_buf(),
                    };
                }
            }
            match std::fs::read_to_string(path) {
                Ok(content) => {
                    match serde_json::from_str::<HashMap<String, HistoryEntry>>(&content) {
                        Ok(mut entries) => {
                            decay_old_entries(&mut entries);
                            let count = entries.len();
                            if count > 0 {
                                info!(count, "routing history loaded");
                            }
                            entries
                        }
                        Err(e) => {
                            warn!(error = %e, "failed to parse routing history, starting fresh");
                            HashMap::new()
                        }
                    }
                }
                Err(e) => {
                    warn!(error = %e, "failed to read routing history, starting fresh");
                    HashMap::new()
                }
            }
        } else {
            HashMap::new()
        };

        Self {
            entries,
            pending_writes: 0,
            last_flush: Instant::now(),
            persist_path: path.to_path_buf(),
        }
    }

    /// Record an outcome for a model+task_type.
    pub fn record(
        &mut self,
        provider: ProviderId,
        model: &str,
        task_type: TaskType,
        success: bool,
        latency_ms: u64,
    ) {
        let key = Self::key(provider, model, task_type);
        let entry = self.entries.entry(key).or_insert_with(|| HistoryEntry {
            successes: 0,
            failures: 0,
            total_latency_ms: 0,
            last_updated: Utc::now(),
        });

        if success {
            entry.successes = entry.successes.saturating_add(1);
        } else {
            entry.failures = entry.failures.saturating_add(1);
        }
        entry.total_latency_ms = entry.total_latency_ms.saturating_add(latency_ms);
        entry.last_updated = Utc::now();

        // Evict oldest entry if cap exceeded
        if self.entries.len() > MAX_ENTRIES {
            if let Some(oldest_key) = self
                .entries
                .iter()
                .min_by_key(|(_, e)| e.last_updated)
                .map(|(k, _)| k.clone())
            {
                self.entries.remove(&oldest_key);
            }
        }

        self.pending_writes += 1;
        if self.pending_writes >= FLUSH_INTERVAL
            || self.last_flush.elapsed() >= std::time::Duration::from_secs(60)
        {
            self.flush();
        }
    }

    /// Get the success rate for a model+task_type. Returns 0.5 for unknown.
    pub fn success_rate(&self, provider: ProviderId, model: &str, task_type: TaskType) -> f64 {
        let key = Self::key(provider, model, task_type);
        self.entries
            .get(&key)
            .map(|e| e.success_rate())
            .unwrap_or(0.5)
    }

    /// Flush pending writes to disk.
    pub fn flush(&mut self) {
        if self.pending_writes == 0 {
            return;
        }
        match serde_json::to_string_pretty(&self.entries) {
            Ok(json) => {
                if let Err(e) = std::fs::write(&self.persist_path, json) {
                    warn!(error = %e, "failed to persist routing history");
                }
            }
            Err(e) => warn!(error = %e, "failed to serialize routing history"),
        }
        self.pending_writes = 0;
        self.last_flush = Instant::now();
    }
}

impl Drop for RoutingHistory {
    fn drop(&mut self) {
        self.flush();
    }
}

/// Halve counts for entries older than DECAY_DAYS.
fn decay_old_entries(entries: &mut HashMap<String, HistoryEntry>) {
    let cutoff = Utc::now() - chrono::Duration::days(DECAY_DAYS);
    for entry in entries.values_mut() {
        if entry.last_updated < cutoff {
            entry.successes /= 2;
            entry.failures /= 2;
            entry.total_latency_ms /= 2;
        }
    }
    // Remove entries that decayed to zero
    entries.retain(|_, e| e.successes > 0 || e.failures > 0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_history_entry_success_rate() {
        let entry = HistoryEntry {
            successes: 8,
            failures: 2,
            total_latency_ms: 10000,
            last_updated: Utc::now(),
        };
        assert!((entry.success_rate() - 0.8).abs() < 0.001);
    }

    #[test]
    fn test_history_entry_empty_returns_neutral() {
        let entry = HistoryEntry {
            successes: 0,
            failures: 0,
            total_latency_ms: 0,
            last_updated: Utc::now(),
        };
        assert!((entry.success_rate() - 0.5).abs() < 0.001);
    }

    #[test]
    fn test_history_entry_avg_latency() {
        let entry = HistoryEntry {
            successes: 5,
            failures: 5,
            total_latency_ms: 10000,
            last_updated: Utc::now(),
        };
        assert_eq!(entry.avg_latency_ms(), 1000);
    }

    #[test]
    fn test_record_and_query() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("routing_history.json");
        let mut history = RoutingHistory::load(&path);

        history.record(
            ProviderId::Anthropic,
            "claude-sonnet-4-6",
            TaskType::Chat,
            true,
            500,
        );
        history.record(
            ProviderId::Anthropic,
            "claude-sonnet-4-6",
            TaskType::Chat,
            true,
            600,
        );
        history.record(
            ProviderId::Anthropic,
            "claude-sonnet-4-6",
            TaskType::Chat,
            false,
            2000,
        );

        let rate = history.success_rate(ProviderId::Anthropic, "claude-sonnet-4-6", TaskType::Chat);
        assert!((rate - 0.6667).abs() < 0.01);

        // Unknown model returns neutral
        let unknown = history.success_rate(ProviderId::OpenaiCodex, "gpt-5.4", TaskType::Plan);
        assert!((unknown - 0.5).abs() < 0.001);
    }

    #[test]
    fn test_persist_and_reload() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("routing_history.json");

        {
            let mut history = RoutingHistory::load(&path);
            history.record(ProviderId::Anthropic, "sonnet", TaskType::Chat, true, 100);
            history.record(ProviderId::Anthropic, "sonnet", TaskType::Chat, true, 200);
            history.flush();
        }

        let history2 = RoutingHistory::load(&path);
        let rate = history2.success_rate(ProviderId::Anthropic, "sonnet", TaskType::Chat);
        assert!((rate - 1.0).abs() < 0.001);
    }

    #[test]
    fn test_load_missing_file_starts_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nonexistent.json");
        let history = RoutingHistory::load(&path);
        let rate = history.success_rate(ProviderId::Anthropic, "x", TaskType::Chat);
        assert!((rate - 0.5).abs() < 0.001);
    }

    #[test]
    fn test_load_corrupt_file_starts_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("routing_history.json");
        std::fs::write(&path, "not valid json{{{").unwrap();
        let history = RoutingHistory::load(&path);
        let rate = history.success_rate(ProviderId::Anthropic, "x", TaskType::Chat);
        assert!((rate - 0.5).abs() < 0.001);
    }

    #[test]
    fn test_decay_old_entries() {
        let mut entries = HashMap::new();
        entries.insert(
            "anthropic:sonnet:chat".into(),
            HistoryEntry {
                successes: 100,
                failures: 10,
                total_latency_ms: 50000,
                last_updated: Utc::now() - chrono::Duration::days(10),
            },
        );
        entries.insert(
            "openai-codex:gpt-5:plan".into(),
            HistoryEntry {
                successes: 20,
                failures: 5,
                total_latency_ms: 10000,
                last_updated: Utc::now(), // recent, no decay
            },
        );

        decay_old_entries(&mut entries);

        // Old entry decayed
        let old = entries.get("anthropic:sonnet:chat").unwrap();
        assert_eq!(old.successes, 50);
        assert_eq!(old.failures, 5);

        // Recent entry untouched
        let recent = entries.get("openai-codex:gpt-5:plan").unwrap();
        assert_eq!(recent.successes, 20);
        assert_eq!(recent.failures, 5);
    }

    #[test]
    fn test_decay_removes_zeroed_entries() {
        let mut entries = HashMap::new();
        entries.insert(
            "anthropic:sonnet:chat".into(),
            HistoryEntry {
                successes: 1,
                failures: 0,
                total_latency_ms: 100,
                last_updated: Utc::now() - chrono::Duration::days(10),
            },
        );

        decay_old_entries(&mut entries);
        // 1/2 = 0, so entry should be removed
        assert!(entries.is_empty());
    }

    #[test]
    fn test_key_format() {
        let key = RoutingHistory::key(ProviderId::Anthropic, "claude-sonnet-4-6", TaskType::Chat);
        assert_eq!(key, "anthropic:claude-sonnet-4-6:chat");
    }
}
