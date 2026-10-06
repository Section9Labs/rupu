//! The token fold: sums the usage ledgers (`usage.jsonl`) of an agentiflow's
//! lead and of every unit it launched into per-model and grand totals. The
//! budget enforcer's `UsageSource` (a later task) is built on it.
//!
//! Each line is a [`rupu_orchestrator::usage_ledger::LedgerRow`]. The fold is
//! tolerant the way the ledger's other readers are: a missing file contributes
//! nothing, and a blank, non-JSON, non-UTF-8 or partially written line is
//! skipped rather than failing the fold (a ledger is appended to by live
//! processes, so the tail line can be mid-write). Rows are de-duplicated by
//! `LedgerRow.id` across ALL the files read, so a line seen twice (a re-read, a
//! mirror replay, one row present in two ledgers) counts once.
//!
//! # Known limitation: dispatched sub-agents are not counted
//!
//! A unit's own LLM calls are in its `usage.jsonl`, but the sub-agents a unit
//! launches through `dispatch_agent` are not: that dispatcher has no usage
//! ledger (a gap that predates the agentiflow fold). The fold therefore
//! UNDERCOUNTS the spend of any unit that dispatches sub-agents, and a
//! `budget.tokens` / `budget.usd` cap trips later than the true spend. See the
//! `dispatch_agent` entry in `TODO.md`.

use std::collections::{BTreeMap, HashSet};
use std::io::BufRead;
use std::path::{Path, PathBuf};

use rupu_orchestrator::usage_ledger::LedgerRow;

/// A token count, in the ledger's own terms.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Tokens {
    /// Input tokens, INCLUDING the cache reads and cache writes below.
    pub input: u64,
    /// Output tokens (already inclusive of any reasoning tokens the provider
    /// bills as output).
    pub output: u64,
    /// Cache reads: a subset of `input`.
    pub cached: u64,
    /// Cache writes: a subset of `input`.
    pub cache_write: u64,
}

impl Tokens {
    /// The tokens a `budget.tokens` cap counts: input plus output. `cached` and
    /// `cache_write` are subsets of `input`, so adding them would double count.
    pub fn billable(&self) -> u64 {
        self.input.saturating_add(self.output)
    }

    /// Add `other` into `self`, saturating rather than overflowing.
    pub fn add(&mut self, other: &Tokens) {
        self.input = self.input.saturating_add(other.input);
        self.output = self.output.saturating_add(other.output);
        self.cached = self.cached.saturating_add(other.cached);
        self.cache_write = self.cache_write.saturating_add(other.cache_write);
    }
}

impl std::ops::AddAssign<&Tokens> for Tokens {
    fn add_assign(&mut self, other: &Tokens) {
        self.add(other);
    }
}

/// The folded usage of a set of ledgers.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TokenTotals {
    /// Tokens per `(provider, model)`.
    pub by_model: BTreeMap<(String, String), Tokens>,
    /// The sum over every model.
    pub total: Tokens,
}

impl TokenTotals {
    fn record(&mut self, row: &LedgerRow) {
        let t = Tokens {
            input: row.input_tokens,
            output: row.output_tokens,
            cached: row.cached_tokens,
            cache_write: row.cache_write_tokens,
        };
        self.by_model
            .entry((row.provider.clone(), row.model.clone()))
            .or_default()
            .add(&t);
        self.total.add(&t);
    }
}

/// Fold every `usage.jsonl` in `usage_jsonl_paths` into [`TokenTotals`].
///
/// Rows are de-duplicated by `LedgerRow.id` across all the files. A path that
/// does not exist (a unit that never called a model) or cannot be read
/// contributes nothing, and an unparseable line is skipped. See the module docs
/// for the `dispatch_agent` undercount.
pub fn fold_tokens(usage_jsonl_paths: &[PathBuf]) -> TokenTotals {
    let mut totals = TokenTotals::default();
    let mut seen: HashSet<String> = HashSet::new();
    for path in usage_jsonl_paths {
        fold_file(path, &mut seen, &mut totals);
    }
    totals
}

fn fold_file(path: &Path, seen: &mut HashSet<String>, totals: &mut TokenTotals) {
    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) => {
            if e.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!(path = %path.display(), error = %e, "usage fold: cannot open ledger");
            }
            return;
        }
    };
    // Split on raw bytes so one non-UTF-8 line cannot fail the whole file.
    for line in std::io::BufReader::new(file).split(b'\n') {
        let line = match line {
            Ok(l) => l,
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "usage fold: read failed");
                return;
            }
        };
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let Ok(row) = serde_json::from_slice::<LedgerRow>(&line) else {
            continue; // garbled, foreign or half-written line
        };
        if seen.insert(row.id.clone()) {
            totals.record(&row);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use rupu_orchestrator::usage_ledger::{LedgerKind, LEDGER_VERSION};
    use std::io::Write as _;

    #[allow(clippy::too_many_arguments)]
    fn row(
        id: &str,
        provider: &str,
        model: &str,
        input: u64,
        output: u64,
        cached: u64,
        cache_write: u64,
    ) -> LedgerRow {
        LedgerRow {
            v: LEDGER_VERSION,
            id: id.into(),
            at: Utc::now(),
            kind: LedgerKind::Turn,
            step_id: None,
            unit_index: None,
            unit_key: None,
            agent_run_id: "ar_1".into(),
            parent_agent_run_id: None,
            transcript: PathBuf::from("/t/x.jsonl"),
            agent: "recon".into(),
            provider: provider.into(),
            model: model.into(),
            input_tokens: input,
            output_tokens: output,
            cached_tokens: cached,
            cache_write_tokens: cache_write,
        }
    }

    fn line(r: &LedgerRow) -> Vec<u8> {
        let mut v = serde_json::to_vec(r).unwrap();
        v.push(b'\n');
        v
    }

    fn write_ledger(path: &Path, rows: &[LedgerRow]) {
        let mut bytes = Vec::new();
        for r in rows {
            bytes.extend(line(r));
        }
        std::fs::write(path, bytes).unwrap();
    }

    fn key(provider: &str, model: &str) -> (String, String) {
        (provider.to_string(), model.to_string())
    }

    #[test]
    fn folds_lead_and_unit_ledgers_per_model_and_in_total() {
        let dir = tempfile::tempdir().unwrap();
        let lead = dir.path().join("lead-usage.jsonl");
        let unit = dir.path().join("unit-usage.jsonl");
        write_ledger(
            &lead,
            &[
                row("01A", "anthropic", "opus", 1000, 100, 400, 50),
                row("01B", "anthropic", "opus", 2000, 200, 0, 0),
            ],
        );
        write_ledger(
            &unit,
            &[
                row("01C", "anthropic", "opus", 500, 50, 10, 5),
                row("01D", "openai", "gpt-x", 300, 30, 0, 0),
            ],
        );

        let got = fold_tokens(&[lead, unit]);

        assert_eq!(
            got.by_model[&key("anthropic", "opus")],
            Tokens {
                input: 3500,
                output: 350,
                cached: 410,
                cache_write: 55,
            }
        );
        assert_eq!(
            got.by_model[&key("openai", "gpt-x")],
            Tokens {
                input: 300,
                output: 30,
                cached: 0,
                cache_write: 0,
            }
        );
        assert_eq!(got.by_model.len(), 2);
        assert_eq!(
            got.total,
            Tokens {
                input: 3800,
                output: 380,
                cached: 410,
                cache_write: 55,
            }
        );
        // Cache tokens are subsets of input: billable is input + output only.
        assert_eq!(got.total.billable(), 3800 + 380);
    }

    #[test]
    fn a_duplicated_row_id_counts_once_across_and_within_files() {
        let dir = tempfile::tempdir().unwrap();
        let lead = dir.path().join("lead.jsonl");
        let unit = dir.path().join("unit.jsonl");
        let dup = row("01DUP", "anthropic", "opus", 700, 70, 0, 0);
        // Twice in the lead's file, and once more in the unit's.
        write_ledger(
            &lead,
            &[
                dup.clone(),
                row("01A", "anthropic", "opus", 100, 10, 0, 0),
                dup.clone(),
            ],
        );
        write_ledger(
            &unit,
            &[dup, row("01B", "anthropic", "opus", 200, 20, 0, 0)],
        );

        let got = fold_tokens(&[lead.clone(), unit.clone()]);
        assert_eq!(got.total.input, 700 + 100 + 200);
        assert_eq!(got.total.output, 70 + 10 + 20);
        assert_eq!(
            got.by_model[&key("anthropic", "opus")],
            got.total,
            "one model: its tokens are the total"
        );

        // Re-reading the same files (a re-listed path) changes nothing.
        let again = fold_tokens(&[lead.clone(), unit, lead]);
        assert_eq!(again, got);
    }

    #[test]
    fn a_missing_path_contributes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("usage.jsonl");
        write_ledger(&real, &[row("01A", "anthropic", "opus", 10, 5, 0, 0)]);
        let missing = dir.path().join("runs").join("run_gone").join("usage.jsonl");

        let got = fold_tokens(&[missing.clone(), real, missing.clone()]);
        assert_eq!(got.total.billable(), 15);

        assert_eq!(fold_tokens(&[missing]), TokenTotals::default());
        assert_eq!(fold_tokens(&[]), TokenTotals::default());
    }

    #[test]
    fn junk_blank_and_non_utf8_lines_are_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("usage.jsonl");
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(&line(&row("01A", "anthropic", "opus", 100, 10, 0, 0)))
            .unwrap();
        f.write_all(b"not json at all\n").unwrap();
        f.write_all(b"\n   \n").unwrap();
        f.write_all(b"{\"v\":1,\"id\":\"01X\"}\n").unwrap(); // JSON, but not a row
        f.write_all(&[0xff, 0xfe, 0xfd, b'\n']).unwrap(); // not UTF-8
        f.write_all(&line(&row("01B", "anthropic", "opus", 200, 20, 0, 0)))
            .unwrap();
        // A half-written tail line (no newline) from a live writer.
        let mut tail = serde_json::to_vec(&row("01C", "anthropic", "opus", 999, 99, 0, 0)).unwrap();
        tail.truncate(tail.len() / 2);
        f.write_all(&tail).unwrap();
        drop(f);

        let got = fold_tokens(&[path]);
        assert_eq!(got.total.input, 300);
        assert_eq!(got.total.output, 30);
    }

    #[test]
    fn a_row_without_cache_write_tokens_reads_as_zero() {
        // Ledgers written before `cache_write_tokens` existed still fold.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("usage.jsonl");
        let mut v = serde_json::to_value(row("01A", "anthropic", "opus", 40, 4, 8, 0)).unwrap();
        v.as_object_mut().unwrap().remove("cache_write_tokens");
        v.as_object_mut().unwrap().remove("cached_tokens");
        std::fs::write(&path, format!("{v}\n")).unwrap();

        let got = fold_tokens(&[path]);
        assert_eq!(
            got.total,
            Tokens {
                input: 40,
                output: 4,
                cached: 0,
                cache_write: 0,
            }
        );
    }

    #[test]
    fn tokens_add_saturates() {
        let mut a = Tokens {
            input: u64::MAX,
            output: 1,
            cached: 0,
            cache_write: 0,
        };
        a += &Tokens {
            input: 5,
            output: 2,
            cached: 3,
            cache_write: 4,
        };
        assert_eq!(a.input, u64::MAX);
        assert_eq!(a.output, 3);
        assert_eq!(a.billable(), u64::MAX);
    }
}
