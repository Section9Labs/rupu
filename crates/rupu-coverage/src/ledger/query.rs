//! The one finding filter (spec "Read path"): [`select`] returns every
//! match, for the CLI and CP; [`query`] pages them by cursor, for the agent
//! tool and MCP. Never a silent cap: a page says how many matched.

use crate::catalog::types::Severity;
use crate::ledger::events::{FindingRecord, FindingScope};
use crate::ledger::tags::Tag;
use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use std::cmp::Reverse;
use std::collections::{BTreeMap, HashSet};

pub const DEFAULT_LIMIT: usize = 50;
pub const MAX_LIMIT: usize = 500;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TagMode {
    #[default]
    All,
    Any,
}

/// What to select. Deserialized from agent/MCP tool input, so unknown
/// fields are refused (a typo must not silently widen the selection).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FindingQuery {
    #[serde(default)]
    pub tags: Vec<Tag>,
    #[serde(default)]
    pub tag_mode: TagMode,
    #[serde(default)]
    pub untagged: bool,
    #[serde(default)]
    pub min_severity: Option<Severity>,
    #[serde(default)]
    pub concern_id: Option<String>,
    #[serde(default)]
    pub file_prefix: Option<String>,
    /// [`query`] only.
    #[serde(default)]
    pub limit: Option<usize>,
    /// [`query`] only: a previous page's `next_cursor`.
    #[serde(default)]
    pub cursor: Option<String>,
    /// Only findings declared by one of these runs. Set by the CLI/CP from a
    /// resolved run scope; never tool input.
    #[serde(skip)]
    pub run_ids: Option<HashSet<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum QueryError {
    #[error("`untagged` cannot be combined with `tags`")]
    UntaggedWithTags,
    #[error("`limit` must be between 1 and {MAX_LIMIT}, got {0}")]
    Limit(usize),
    #[error("`cursor` is not one this query returned: {0}")]
    Cursor(String),
}

/// Sort rank: critical (4) first.
pub fn severity_rank(s: Severity) -> u8 {
    match s {
        Severity::Critical => 4,
        Severity::High => 3,
        Severity::Medium => 2,
        Severity::Low => 1,
        Severity::Info => 0,
    }
}

fn matches(r: &FindingRecord, q: &FindingQuery) -> bool {
    if q.untagged && !r.tags.is_empty() {
        return false;
    }
    if !q.tags.is_empty() {
        let has = |t: &Tag| r.tags.contains(t);
        let ok = match q.tag_mode {
            TagMode::All => q.tags.iter().all(has),
            TagMode::Any => q.tags.iter().any(has),
        };
        if !ok {
            return false;
        }
    }
    if q.min_severity
        .is_some_and(|min| severity_rank(r.severity) < severity_rank(min))
    {
        return false;
    }
    if q.concern_id
        .as_deref()
        .is_some_and(|c| r.concern_id.as_deref() != Some(c))
    {
        return false;
    }
    if let Some(prefix) = q.file_prefix.as_deref() {
        if !r
            .file_path
            .as_deref()
            .is_some_and(|f| f.starts_with(prefix))
        {
            return false;
        }
    }
    if let Some(runs) = &q.run_ids {
        if !runs.contains(&r.declared_by.run_id) {
            return false;
        }
    }
    true
}

type SortKey = (Reverse<u8>, Reverse<DateTime<Utc>>, String);

fn sort_key(r: &FindingRecord) -> SortKey {
    (
        Reverse(severity_rank(r.severity)),
        Reverse(r.declared_at),
        r.id.clone(),
    )
}

/// Every item whose record matches, ordered severity → newest → id.
pub fn select<'a, T>(
    items: &'a [T],
    record_of: impl Fn(&T) -> &FindingRecord,
    q: &FindingQuery,
) -> Result<Vec<&'a T>, QueryError> {
    if q.untagged && !q.tags.is_empty() {
        return Err(QueryError::UntaggedWithTags);
    }
    // `filter` and `sort_by_cached_key` hand the closures `&&T`: deref once.
    let mut out: Vec<&T> = items.iter().filter(|i| matches(record_of(*i), q)).collect();
    out.sort_by_cached_key(|i| sort_key(record_of(*i)));
    Ok(out)
}

/// A slim row: what an agent or a table needs, never the report body.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FindingRow {
    pub id: String,
    /// The report's title on a full-profile finding, else its summary.
    pub title: String,
    pub severity: Severity,
    pub scope: FindingScope,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub concern_id: Option<String>,
    pub tags: Vec<Tag>,
    pub declared_at: DateTime<Utc>,
    pub run_id: String,
}

impl From<&FindingRecord> for FindingRow {
    fn from(r: &FindingRecord) -> Self {
        let location = match (&r.file_path, r.line_range) {
            (Some(f), Some([a, b])) => Some(format!("{f}:{a}-{b}")),
            (Some(f), None) => Some(f.clone()),
            (None, _) => r.target_ref.clone(),
        };
        FindingRow {
            id: r.id.clone(),
            title: r
                .report
                .as_ref()
                .map(|rep| rep.title.clone())
                .unwrap_or_else(|| r.summary.clone()),
            severity: r.severity,
            scope: r.scope,
            location,
            concern_id: r.concern_id.clone(),
            tags: r.tags.clone(),
            declared_at: r.declared_at,
            run_id: r.declared_by.run_id.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Page {
    pub rows: Vec<FindingRow>,
    /// Pass back as `cursor` for the next page; `None` on the last page.
    pub next_cursor: Option<String>,
    /// Matches across all pages.
    pub total: usize,
}

fn cursor_of(r: &FindingRecord) -> String {
    format!(
        "{}|{}|{}",
        severity_rank(r.severity),
        r.declared_at.to_rfc3339_opts(SecondsFormat::Nanos, true),
        r.id
    )
}

fn parse_cursor(c: &str) -> Result<SortKey, QueryError> {
    let bad = || QueryError::Cursor(c.to_string());
    let mut parts = c.splitn(3, '|');
    let rank: u8 = parts.next().and_then(|p| p.parse().ok()).ok_or_else(bad)?;
    let at = parts
        .next()
        .and_then(|p| DateTime::parse_from_rfc3339(p).ok())
        .ok_or_else(bad)?
        .with_timezone(&Utc);
    let id = parts.next().filter(|p| !p.is_empty()).ok_or_else(bad)?;
    Ok((Reverse(rank), Reverse(at), id.to_string()))
}

/// One page of matches, after `q.cursor`. A cursor is a sort key, not an
/// offset, so findings appended while paging never shift or repeat rows.
pub fn query(records: &[FindingRecord], q: &FindingQuery) -> Result<Page, QueryError> {
    let limit = q.limit.unwrap_or(DEFAULT_LIMIT);
    if limit == 0 || limit > MAX_LIMIT {
        return Err(QueryError::Limit(limit));
    }
    let all = select(records, |r| r, q)?;
    let total = all.len();
    let start = match q.cursor.as_deref() {
        None => 0,
        Some(c) => {
            let key = parse_cursor(c)?;
            all.partition_point(|r| sort_key(r) <= key)
        }
    };
    let page: Vec<&FindingRecord> = all[start..].iter().take(limit).copied().collect();
    let next_cursor = if start + page.len() < total {
        page.last().map(|r| cursor_of(r))
    } else {
        None
    };
    Ok(Page {
        rows: page.into_iter().map(FindingRow::from).collect(),
        next_cursor,
        total,
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TagCount {
    pub tag: Tag,
    pub count: usize,
}

/// Each tag on `records` and how many carry it, most used first.
pub fn tags_in_use<'a>(records: impl IntoIterator<Item = &'a FindingRecord>) -> Vec<TagCount> {
    let mut counts: BTreeMap<&Tag, usize> = BTreeMap::new();
    for r in records {
        for t in &r.tags {
            *counts.entry(t).or_default() += 1;
        }
    }
    let mut v: Vec<TagCount> = counts
        .into_iter()
        .map(|(tag, count)| TagCount {
            tag: tag.clone(),
            count,
        })
        .collect();
    v.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.tag.cmp(&b.tag)));
    v
}

/// The agent tool's and MCP's answer: one page plus the vocabulary in use
/// across `records` (the whole workspace, not just the matches).
pub fn query_response(
    records: &[FindingRecord],
    q: &FindingQuery,
) -> Result<serde_json::Value, QueryError> {
    let page = query(records, q)?;
    Ok(serde_json::json!({
        "rows": page.rows,
        "next_cursor": page.next_cursor,
        "total": page.total,
        "tags_in_use": tags_in_use(records),
    }))
}

/// Input schema shared by `query_findings` and `findings.query`.
pub fn query_input_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "tags": { "type": "array", "items": { "type": "string" }, "description": "Only findings carrying these tags." },
            "tag_mode": { "type": "string", "enum": ["all", "any"], "description": "With `tags`: findings carrying all of them (default) or any of them." },
            "untagged": { "type": "boolean", "description": "Only findings with no tags. Cannot be combined with `tags`." },
            "min_severity": { "type": "string", "enum": ["info", "low", "medium", "high", "critical"], "description": "Only this severity and worse." },
            "concern_id": { "type": "string", "description": "Only findings for this concern id." },
            "file_prefix": { "type": "string", "description": "Only findings whose file_path starts with this." },
            "limit": { "type": "integer", "minimum": 1, "maximum": MAX_LIMIT, "description": "Rows per page (default 50)." },
            "cursor": { "type": "string", "description": "`next_cursor` from the previous page." }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::events::{Attribution, FindingEvidence, Surface};
    use crate::ledger::tags::parse_tags;
    use chrono::TimeZone;

    fn rec(id: &str, sev: Severity, minute: u32, tags: &[&str]) -> FindingRecord {
        FindingRecord {
            id: id.into(),
            file_path: Some(format!("src/{id}.rs")),
            line_range: Some([3, 4]),
            target_ref: None,
            scope: FindingScope::Line,
            summary: format!("summary {id}"),
            severity: sev,
            concern_id: Some("injection".into()),
            evidence: FindingEvidence {
                code_excerpt: None,
                rationale: "r".into(),
                references: vec![],
            },
            declared_by: Attribution {
                run_id: format!("run_{id}"),
                model: "m".into(),
                surface: Surface::Workflow,
                codename: None,
                agent: None,
                provider: None,
            },
            declared_at: Utc.with_ymd_and_hms(2026, 10, 6, 12, minute, 0).unwrap(),
            profile: crate::report::FindingProfile::Summary,
            report: None,
            tags: parse_tags(tags).unwrap(),
        }
    }

    fn fixture() -> Vec<FindingRecord> {
        vec![
            rec("fnd_low", Severity::Low, 1, &["class:sqli"]),
            rec(
                "fnd_crit",
                Severity::Critical,
                2,
                &["class:sqli", "needs-poc"],
            ),
            rec("fnd_high_old", Severity::High, 3, &[]),
            rec("fnd_high_new", Severity::High, 9, &["needs-poc"]),
        ]
    }

    fn q() -> FindingQuery {
        FindingQuery::default()
    }

    fn ids(v: &[&FindingRecord]) -> Vec<String> {
        v.iter().map(|r| r.id.clone()).collect()
    }

    #[test]
    fn select_orders_by_severity_then_newest_first() {
        let f = fixture();
        let all = select(&f, |r| r, &q()).unwrap();
        assert_eq!(
            ids(&all),
            ["fnd_crit", "fnd_high_new", "fnd_high_old", "fnd_low"]
        );
    }

    #[test]
    fn tag_filters_all_any_and_untagged() {
        let f = fixture();
        let both = FindingQuery {
            tags: parse_tags(&["class:sqli", "needs-poc"]).unwrap(),
            ..q()
        };
        assert_eq!(ids(&select(&f, |r| r, &both).unwrap()), ["fnd_crit"]);
        let any = FindingQuery {
            tag_mode: TagMode::Any,
            ..both.clone()
        };
        assert_eq!(
            ids(&select(&f, |r| r, &any).unwrap()),
            ["fnd_crit", "fnd_high_new", "fnd_low"]
        );
        let untagged = FindingQuery {
            untagged: true,
            ..q()
        };
        assert_eq!(
            ids(&select(&f, |r| r, &untagged).unwrap()),
            ["fnd_high_old"]
        );
        let bad = FindingQuery {
            untagged: true,
            ..both
        };
        assert_eq!(
            select(&f, |r| r, &bad).unwrap_err(),
            QueryError::UntaggedWithTags
        );
    }

    #[test]
    fn other_filters_narrow() {
        let f = fixture();
        let high_up = FindingQuery {
            min_severity: Some(Severity::High),
            ..q()
        };
        assert_eq!(select(&f, |r| r, &high_up).unwrap().len(), 3);
        let file = FindingQuery {
            file_prefix: Some("src/fnd_low".into()),
            ..q()
        };
        assert_eq!(ids(&select(&f, |r| r, &file).unwrap()), ["fnd_low"]);
        let run = FindingQuery {
            run_ids: Some(["run_fnd_crit".to_string()].into()),
            ..q()
        };
        assert_eq!(ids(&select(&f, |r| r, &run).unwrap()), ["fnd_crit"]);
        let concern = FindingQuery {
            concern_id: Some("other".into()),
            ..q()
        };
        assert!(select(&f, |r| r, &concern).unwrap().is_empty());
    }

    #[test]
    fn pages_follow_the_cursor_and_stay_stable_under_appends() {
        let mut f = fixture();
        let p1 = query(
            &f,
            &FindingQuery {
                limit: Some(2),
                ..q()
            },
        )
        .unwrap();
        assert_eq!(p1.total, 4);
        let p1_ids: Vec<&str> = p1.rows.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(p1_ids, ["fnd_crit", "fnd_high_new"]);
        // A newer critical finding lands while paging: it sorts before the
        // cursor, so the next page neither repeats nor skips a row.
        f.push(rec("fnd_crit_new", Severity::Critical, 30, &[]));
        let p2 = query(
            &f,
            &FindingQuery {
                limit: Some(2),
                cursor: p1.next_cursor.clone(),
                ..q()
            },
        )
        .unwrap();
        let p2_ids: Vec<&str> = p2.rows.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(p2_ids, ["fnd_high_old", "fnd_low"]);
        assert_eq!(p2.next_cursor, None);
    }

    #[test]
    fn limits_and_cursors_are_validated() {
        let f = fixture();
        assert_eq!(
            query(
                &f,
                &FindingQuery {
                    limit: Some(0),
                    ..q()
                }
            )
            .unwrap_err(),
            QueryError::Limit(0)
        );
        assert_eq!(
            query(
                &f,
                &FindingQuery {
                    limit: Some(501),
                    ..q()
                }
            )
            .unwrap_err(),
            QueryError::Limit(501)
        );
        assert!(matches!(
            query(
                &f,
                &FindingQuery {
                    cursor: Some("garbage".into()),
                    ..q()
                }
            ),
            Err(QueryError::Cursor(_))
        ));
        assert_eq!(query(&f, &q()).unwrap().rows.len(), 4);
    }

    #[test]
    fn rows_are_slim_and_located() {
        let f = fixture();
        let row = FindingRow::from(&f[0]);
        assert_eq!(row.title, "summary fnd_low");
        assert_eq!(row.location.as_deref(), Some("src/fnd_low.rs:3-4"));
        assert_eq!(row.run_id, "run_fnd_low");
    }

    #[test]
    fn tags_in_use_counts_by_count_then_name() {
        let f = fixture();
        let counts = tags_in_use(&f);
        let got: Vec<(&str, usize)> = counts.iter().map(|c| (c.tag.as_str(), c.count)).collect();
        assert_eq!(got, [("class:sqli", 2), ("needs-poc", 2)]);
    }

    #[test]
    fn query_input_rejects_unknown_fields() {
        let ok: FindingQuery =
            serde_json::from_value(serde_json::json!({"tags": ["Needs-POC"], "tag_mode": "any"}))
                .unwrap();
        assert_eq!(ok.tags[0].as_str(), "needs-poc");
        assert!(serde_json::from_value::<FindingQuery>(serde_json::json!({"tag": ["x"]})).is_err());
        assert!(
            serde_json::from_value::<FindingQuery>(serde_json::json!({"run_ids": ["x"]})).is_err()
        );
    }
}
