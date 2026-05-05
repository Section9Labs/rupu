//! Token-spend aggregation across one or more transcripts.
//!
//! Walks the JSONL event stream of each transcript, sums every
//! `Usage` event keyed by `(provider, model, agent)`, and returns a
//! flat list of rows. The CLI's `rupu usage` subcommand renders
//! these as a table.
//!
//! `Usage` events are emitted by the agent runtime once per turn.
//! The total run-level number reported in `RunComplete.total_tokens`
//! is intentionally NOT used here — it doesn't separate input from
//! output, and a multi-turn run's per-turn `Usage` sum is the
//! authoritative breakdown.

use crate::event::Event;
use crate::reader::JsonlReader;
use chrono::{DateTime, Utc};
use std::collections::BTreeMap;
use std::path::Path;

/// One row in an aggregated usage report. The `(provider, model,
/// agent)` triple is the natural primary key — the same agent run
/// against the same model twice rolls up into one row.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UsageRow {
    pub provider: String,
    pub model: String,
    pub agent: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached_tokens: u64,
    /// How many distinct transcripts contributed to this row.
    pub runs: u64,
}

impl UsageRow {
    pub fn total_tokens(&self) -> u64 {
        self.input_tokens + self.output_tokens
    }
}

/// Optional time-window filter for [`aggregate`]. `since` and
/// `until` compare against each run's `RunStart.started_at`. Either
/// bound may be `None`.
#[derive(Debug, Clone, Copy, Default)]
pub struct TimeWindow {
    pub since: Option<DateTime<Utc>>,
    pub until: Option<DateTime<Utc>>,
}

impl TimeWindow {
    fn includes(&self, ts: DateTime<Utc>) -> bool {
        if let Some(s) = self.since {
            if ts < s {
                return false;
            }
        }
        if let Some(u) = self.until {
            if ts > u {
                return false;
            }
        }
        true
    }
}

/// Walk every transcript at `paths` and return the aggregated rows.
/// Unreadable / unparseable transcripts are skipped silently — same
/// policy as `rupu transcript list`.
///
/// Within each transcript:
/// - We need a `RunStart` to know `(provider, model, agent)` and
///   `started_at`. A transcript without `RunStart` is skipped.
/// - Every `Usage` event sums into the row keyed by that triple.
///   If a `Usage` event carries different `provider` / `model`
///   from the run-start values (e.g. mid-run model swap), the
///   event's own values win — so a model migration shows up
///   correctly in the breakdown.
/// - Each transcript bumps the `runs` counter on every row it
///   contributed to (typically 1).
pub fn aggregate<P: AsRef<Path>>(paths: &[P], window: TimeWindow) -> Vec<UsageRow> {
    let mut by_key: BTreeMap<(String, String, String), UsageRow> = BTreeMap::new();
    for path in paths {
        let Some(rows) = aggregate_one(path.as_ref(), window) else {
            continue;
        };
        for (key, row) in rows {
            let entry = by_key.entry(key.clone()).or_insert_with(|| UsageRow {
                provider: key.0,
                model: key.1,
                agent: key.2,
                ..UsageRow::default()
            });
            entry.input_tokens += row.input_tokens;
            entry.output_tokens += row.output_tokens;
            entry.cached_tokens += row.cached_tokens;
            entry.runs += row.runs;
        }
    }
    let mut out: Vec<UsageRow> = by_key.into_values().collect();
    // Highest spend first; tiebreak by provider/model/agent for
    // deterministic ordering.
    out.sort_by(|a, b| {
        b.total_tokens().cmp(&a.total_tokens()).then_with(|| {
            (a.provider.as_str(), a.model.as_str(), a.agent.as_str()).cmp(&(
                b.provider.as_str(),
                b.model.as_str(),
                b.agent.as_str(),
            ))
        })
    });
    out
}

/// Aggregate a single transcript. Returns `None` if the file is
/// missing a `RunStart` or falls outside the time window. Returns
/// `Some(empty)` if the run had no `Usage` events but did start —
/// callers can ignore empty rows.
fn aggregate_one(
    path: &Path,
    window: TimeWindow,
) -> Option<BTreeMap<(String, String, String), UsageRow>> {
    let iter = JsonlReader::iter(path).ok()?;
    let mut start_provider: Option<String> = None;
    let mut start_model: Option<String> = None;
    let mut start_agent: Option<String> = None;
    let mut started_at: Option<DateTime<Utc>> = None;
    let mut by_key: BTreeMap<(String, String, String), UsageRow> = BTreeMap::new();

    for ev in iter {
        match ev {
            Ok(Event::RunStart {
                provider,
                model,
                agent,
                started_at: ts,
                ..
            }) => {
                start_provider = Some(provider);
                start_model = Some(model);
                start_agent = Some(agent);
                started_at = Some(ts);
            }
            Ok(Event::Usage {
                provider,
                model,
                input_tokens,
                output_tokens,
                cached_tokens,
            }) => {
                // Anchor on the run-start agent, but let the Usage
                // event override provider/model when they differ
                // (mid-run swap).
                let agent = match &start_agent {
                    Some(a) => a.clone(),
                    None => continue, // no run-start yet; skip the orphan
                };
                let key = (provider, model, agent);
                let row = by_key.entry(key).or_default();
                row.input_tokens += input_tokens as u64;
                row.output_tokens += output_tokens as u64;
                row.cached_tokens += cached_tokens as u64;
            }
            // Ignore everything else — including parse errors that
            // bubble up from truncated lines.
            _ => {}
        }
    }

    let started_at = started_at?;
    if !window.includes(started_at) {
        return None;
    }
    let provider = start_provider?;
    let model = start_model?;
    let agent = start_agent?;

    // If the run produced no Usage events, still emit one zero-token
    // row keyed by (provider, model, agent) so the run is visible in
    // the output. Bumps `runs` so call counts stay accurate.
    if by_key.is_empty() {
        by_key.insert(
            (provider, model, agent),
            UsageRow {
                runs: 1,
                ..UsageRow::default()
            },
        );
        return Some(by_key);
    }

    // Backfill the (provider, model, agent) on every row + count this
    // transcript as 1 run for each row it contributed to.
    let mut filled: BTreeMap<(String, String, String), UsageRow> = BTreeMap::new();
    for ((p, m, a), mut r) in by_key {
        r.provider = p.clone();
        r.model = m.clone();
        r.agent = a.clone();
        r.runs = 1;
        filled.insert((p, m, a), r);
    }
    // Discard the borrow checker's warning about unused start_*
    // when we reach here; they were only used to anchor the
    // agent and ts above.
    let _ = (provider, model, agent, started_at);
    Some(filled)
}

#[allow(unused_imports)]
#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{Event, RunMode, RunStatus};
    use crate::writer::JsonlWriter;
    use chrono::Utc;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn write_transcript(dir: &Path, name: &str, events: &[Event]) -> PathBuf {
        let path = dir.join(name);
        let mut w = JsonlWriter::create(&path).unwrap();
        for ev in events {
            w.write(ev).unwrap();
        }
        path
    }

    fn run_start(agent: &str, provider: &str, model: &str) -> Event {
        Event::RunStart {
            run_id: format!("run_{agent}"),
            workspace_id: "ws".into(),
            agent: agent.into(),
            provider: provider.into(),
            model: model.into(),
            started_at: Utc::now(),
            mode: RunMode::Bypass,
        }
    }

    fn usage(provider: &str, model: &str, input: u32, output: u32) -> Event {
        Event::Usage {
            provider: provider.into(),
            model: model.into(),
            input_tokens: input,
            output_tokens: output,
            cached_tokens: 0,
        }
    }

    fn run_complete() -> Event {
        Event::RunComplete {
            run_id: "run".into(),
            status: RunStatus::Ok,
            total_tokens: 0,
            duration_ms: 100,
            error: None,
        }
    }

    #[test]
    fn empty_input_yields_no_rows() {
        let rows = aggregate::<PathBuf>(&[], TimeWindow::default());
        assert!(rows.is_empty());
    }

    #[test]
    fn single_run_with_two_usage_events_sums() {
        let tmp = TempDir::new().unwrap();
        let p = write_transcript(
            tmp.path(),
            "a.jsonl",
            &[
                run_start("reviewer", "anthropic", "claude-sonnet-4-6"),
                usage("anthropic", "claude-sonnet-4-6", 100, 50),
                usage("anthropic", "claude-sonnet-4-6", 200, 75),
                run_complete(),
            ],
        );
        let rows = aggregate(&[p], TimeWindow::default());
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].provider, "anthropic");
        assert_eq!(rows[0].model, "claude-sonnet-4-6");
        assert_eq!(rows[0].agent, "reviewer");
        assert_eq!(rows[0].input_tokens, 300);
        assert_eq!(rows[0].output_tokens, 125);
        assert_eq!(rows[0].runs, 1);
    }

    #[test]
    fn rolls_up_across_transcripts_keyed_by_provider_model_agent() {
        let tmp = TempDir::new().unwrap();
        let a = write_transcript(
            tmp.path(),
            "a.jsonl",
            &[
                run_start("reviewer", "anthropic", "claude-sonnet-4-6"),
                usage("anthropic", "claude-sonnet-4-6", 100, 50),
            ],
        );
        let b = write_transcript(
            tmp.path(),
            "b.jsonl",
            &[
                run_start("reviewer", "anthropic", "claude-sonnet-4-6"),
                usage("anthropic", "claude-sonnet-4-6", 50, 25),
            ],
        );
        let c = write_transcript(
            tmp.path(),
            "c.jsonl",
            &[
                run_start("fixer", "anthropic", "claude-sonnet-4-6"),
                usage("anthropic", "claude-sonnet-4-6", 10, 5),
            ],
        );
        let rows = aggregate(&[a, b, c], TimeWindow::default());
        assert_eq!(rows.len(), 2, "two unique (provider,model,agent) triples");
        // Sorted by total_tokens desc — reviewer row first.
        assert_eq!(rows[0].agent, "reviewer");
        assert_eq!(rows[0].input_tokens, 150);
        assert_eq!(rows[0].output_tokens, 75);
        assert_eq!(rows[0].runs, 2);
        assert_eq!(rows[1].agent, "fixer");
        assert_eq!(rows[1].runs, 1);
    }

    #[test]
    fn time_window_filters_by_started_at() {
        let tmp = TempDir::new().unwrap();
        let now = Utc::now();
        let mut early = run_start("a", "anthropic", "m");
        if let Event::RunStart {
            ref mut started_at,
            ..
        } = early
        {
            *started_at = now - chrono::Duration::days(7);
        }
        let mut late = run_start("a", "anthropic", "m");
        if let Event::RunStart {
            ref mut started_at,
            ..
        } = late
        {
            *started_at = now;
        }
        let early_p = write_transcript(
            tmp.path(),
            "early.jsonl",
            &[early, usage("anthropic", "m", 1, 1)],
        );
        let late_p = write_transcript(
            tmp.path(),
            "late.jsonl",
            &[late, usage("anthropic", "m", 1, 1)],
        );

        let window = TimeWindow {
            since: Some(now - chrono::Duration::days(1)),
            until: None,
        };
        let rows = aggregate(&[early_p, late_p], window);
        assert_eq!(rows.len(), 1, "only the recent run is included");
        assert_eq!(rows[0].input_tokens, 1);
        assert_eq!(rows[0].runs, 1);
    }

    #[test]
    fn run_with_no_usage_events_still_counts_as_one_run() {
        let tmp = TempDir::new().unwrap();
        let p = write_transcript(
            tmp.path(),
            "no-usage.jsonl",
            &[
                run_start("a", "anthropic", "m"),
                // No Usage events between start and complete.
                run_complete(),
            ],
        );
        let rows = aggregate(&[p], TimeWindow::default());
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].runs, 1);
        assert_eq!(rows[0].input_tokens, 0);
        assert_eq!(rows[0].output_tokens, 0);
    }

    #[test]
    fn missing_run_start_skips_the_transcript() {
        let tmp = TempDir::new().unwrap();
        let p = write_transcript(
            tmp.path(),
            "broken.jsonl",
            &[
                // No RunStart — only Usage. This should be skipped
                // silently rather than counted under empty
                // (provider,model,agent).
                usage("anthropic", "m", 100, 100),
            ],
        );
        let rows = aggregate(&[p], TimeWindow::default());
        assert!(rows.is_empty());
    }
}
