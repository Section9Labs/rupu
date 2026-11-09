//! `GET /api/agentiflows/:id/usage-timeline` — every model call an
//! agentiflow made, the lead's and every unit's, as one per-turn token series
//! in the shape a run's usage timeline has (`TurnPoint`), so the web draws
//! the same Token flow on an agentiflow as on a run.
//!
//! The source is the flow's own usage ledger (`<flow dir>/usage.jsonl`): the
//! lead writes its calls there, and every unit process copies its calls in as
//! rollup rows (W7, L5). Points are in call order (`at`), labelled by the
//! lead round they belong to: the lead's from its round transcript
//! (`lead/transcript.r<N>.jsonl`), a unit's from the round that launched it
//! (the children ledger's `round`), and a row neither names — a deeper
//! child's, or one from a ledger written before rounds were recorded — from
//! the lead round running when it was made.

use crate::usage::TurnPoint;
use rupu_runtime::usage_ledger::{LedgerKind, LedgerRow};
use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::Path;

/// The lead round a lead transcript belongs to: `transcript.r<N>.jsonl`.
fn lead_round(transcript: &Path) -> Option<u32> {
    let name = transcript.file_name()?.to_str()?;
    name.strip_prefix("transcript.r")?.strip_suffix(".jsonl")?.parse().ok()
}

/// The flow's ledger rows in file order; malformed lines are skipped.
fn read_rows(flow_dir: &Path) -> Vec<LedgerRow> {
    let Ok(file) = std::fs::File::open(flow_dir.join("usage.jsonl")) else {
        return Vec::new();
    };
    BufReader::new(file)
        .lines()
        .map_while(Result::ok)
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str::<LedgerRow>(&l).ok())
        .collect()
}

/// The flow's per-turn series. `rounds` maps a child's run id to the lead
/// round that launched it.
pub fn timeline(flow_dir: &Path, rounds: &HashMap<String, u32>) -> Vec<TurnPoint> {
    let rows = read_rows(flow_dir);

    // Each call's ordinal within its transcript, in file order (the order
    // the transcript itself recorded them); a compaction call has none.
    let mut ordinal: HashMap<&Path, u64> = HashMap::new();
    let mut tagged: Vec<(&LedgerRow, Option<u64>, Option<u32>)> = rows
        .iter()
        .map(|r| {
            let turn = (r.kind == LedgerKind::Turn).then(|| {
                let n = ordinal.entry(r.transcript.as_path()).or_insert(0);
                *n += 1;
                *n
            });
            let round = lead_round(&r.transcript)
                .filter(|_| r.parent_agent_run_id.is_none())
                .or_else(|| rounds.get(&r.agent_run_id).copied())
                .or_else(|| r.parent_agent_run_id.as_ref().and_then(|p| rounds.get(p).copied()));
            (r, turn, round)
        })
        .collect();
    tagged.sort_by_key(|(r, _, _)| r.at);

    // A row with no round of its own takes the lead round running at the time.
    let mut current: Option<u32> = None;
    let mut points = Vec::with_capacity(tagged.len());
    for (r, turn, round) in tagged {
        if r.parent_agent_run_id.is_none() {
            if let Some(n) = lead_round(&r.transcript) {
                current = Some(n);
            }
        }
        let round = round.or(current).unwrap_or(0);
        points.push(TurnPoint {
            turn: points.len() as u64 + 1,
            label: format!("round {round}"),
            tokens_in: r.input_tokens,
            tokens_out: r.output_tokens,
            tokens_cached: r.cached_tokens,
            tokens_cache_write: r.cache_write_tokens,
            at: Some(r.at),
            transcript: Some(r.transcript.to_string_lossy().into_owned()),
            transcript_turn: turn,
        });
    }
    points
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn row(at: &str, kind: &str, run: &str, parent: Option<&str>, transcript: &str, input: u64) -> String {
        let mut v = serde_json::json!({
            "v": 1, "id": format!("id-{at}"), "at": at, "kind": kind,
            "agent_run_id": run, "transcript": transcript, "agent": "a",
            "provider": "anthropic", "model": "m",
            "input_tokens": input, "output_tokens": 10, "cached_tokens": 5, "cache_write_tokens": 1,
        });
        if let Some(p) = parent {
            v["parent_agent_run_id"] = serde_json::json!(p);
            v["rollup"] = serde_json::json!(true);
        }
        v.to_string()
    }

    fn flow(lines: &[String]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let mut f = std::fs::File::create(dir.path().join("usage.jsonl")).unwrap();
        for l in lines {
            writeln!(f, "{l}").unwrap();
        }
        writeln!(f, "not json").unwrap();
        dir
    }

    #[test]
    fn folds_lead_and_unit_calls_in_call_order_labelled_by_round() {
        let lead0 = "/h/agentiflows/af_x/lead/transcript.r0.jsonl";
        let lead1 = "/h/agentiflows/af_x/lead/transcript.r1.jsonl";
        let unit = "/h/transcripts/run_u.jsonl";
        let deep = "/h/transcripts/run_deep.jsonl";
        let dir = flow(&[
            row("2026-10-15T18:00:00Z", "turn", "af_x", None, lead0, 100),
            row("2026-10-15T18:03:00Z", "turn", "run_u", Some("af_x"), unit, 300),
            row("2026-10-15T18:01:00Z", "turn", "run_u", Some("af_x"), unit, 200),
            row("2026-10-15T18:04:00Z", "compaction", "run_u", Some("af_x"), unit, 50),
            row("2026-10-15T19:00:00Z", "turn", "af_x", None, lead1, 400),
            row("2026-10-15T19:01:00Z", "turn", "run_deep", Some("run_u"), deep, 500),
        ]);
        let rounds = HashMap::from([("run_u".to_string(), 0)]);
        let pts = timeline(dir.path(), &rounds);

        let summary: Vec<(u64, &str, u64, Option<u64>)> = pts
            .iter()
            .map(|p| (p.turn, p.label.as_str(), p.tokens_in, p.transcript_turn))
            .collect();
        assert_eq!(
            summary,
            vec![
                (1, "round 0", 100, Some(1)),
                // unit rows in time order; their ordinals follow the file
                (2, "round 0", 200, Some(2)),
                (3, "round 0", 300, Some(1)),
                (4, "round 0", 50, None),
                (5, "round 1", 400, Some(1)),
                // a grandchild with no round of its own: its parent's round
                (6, "round 0", 500, Some(1)),
            ]
        );
        assert_eq!(pts[1].transcript.as_deref(), Some(unit));
        assert_eq!(pts[1].tokens_cached, 5);
        assert_eq!(pts[1].tokens_cache_write, 1);
        assert_eq!(pts[0].at.unwrap().to_rfc3339(), "2026-10-15T18:00:00+00:00");
    }

    #[test]
    fn a_call_no_round_names_takes_the_lead_round_running_then() {
        let dir = flow(&[
            row("2026-10-15T18:00:00Z", "turn", "af_x", None, "/l/transcript.r0.jsonl", 1),
            row("2026-10-15T18:30:00Z", "turn", "af_x", None, "/l/transcript.r1.jsonl", 2),
            row("2026-10-15T18:31:00Z", "turn", "run_old", Some("af_x"), "/t/run_old.jsonl", 3),
        ]);
        let pts = timeline(dir.path(), &HashMap::new());
        assert_eq!(pts.iter().map(|p| p.label.as_str()).collect::<Vec<_>>(), ["round 0", "round 1", "round 1"]);
    }

    #[test]
    fn no_ledger_is_an_empty_series() {
        let dir = tempfile::tempdir().unwrap();
        assert!(timeline(dir.path(), &HashMap::new()).is_empty());
    }
}
