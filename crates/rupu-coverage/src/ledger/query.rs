//! The paged read of the findings ledger, for the agent tool and MCP:
//! [`query`] parses a one-line query string (`ledger::query_lang`), selects
//! with the one evaluator (`ledger::finding_filter`), and pages the matches
//! by cursor. Never a silent cap: a page says how many matched.

use crate::catalog::types::Severity;
use crate::ledger::events::{FindingRecord, FindingScope};
use crate::ledger::finding_filter::{check_available, select, FindingView, RunScopes};
use crate::ledger::query_lang::parse_query;
use crate::ledger::tags::Tag;
use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use std::cmp::Reverse;
use std::collections::BTreeMap;

pub const DEFAULT_LIMIT: usize = 50;
pub const MAX_LIMIT: usize = 500;

/// Agent tool / MCP input: a findings query string plus paging. Unknown
/// fields are refused (a typo must not silently widen the selection).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FindingQuery {
    /// A findings query (`severity>=high tag:needs-poc -has:poc`); empty
    /// matches everything.
    #[serde(default)]
    pub q: String,
    #[serde(default)]
    pub limit: Option<usize>,
    /// A previous page's `next_cursor`.
    #[serde(default)]
    pub cursor: Option<String>,
    /// Every match in one answer, unpaged (for a workflow `for_each` over the
    /// rows). Refused together with `limit` or `cursor`.
    #[serde(default)]
    pub all: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum QueryError {
    #[error("{0}")]
    Parse(#[from] crate::ledger::query_lang::ParseError),
    #[error("{0}")]
    Unavailable(#[from] crate::ledger::finding_filter::Unavailable),
    #[error("`limit` must be between 1 and {MAX_LIMIT}, got {0}")]
    Limit(usize),
    #[error("`cursor` is not one this query returned: {0}")]
    Cursor(String),
    #[error("`all` returns every match in one answer, so it takes no `limit` or `cursor`")]
    AllWithPaging,
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

type SortKey = (Reverse<u8>, Reverse<DateTime<Utc>>, String);

fn sort_key(r: &FindingRecord) -> SortKey {
    (
        Reverse(severity_rank(r.severity)),
        Reverse(r.declared_at),
        r.id.clone(),
    )
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
    if q.all && (q.limit.is_some() || q.cursor.is_some()) {
        return Err(QueryError::AllWithPaging);
    }
    let limit = q.limit.unwrap_or(DEFAULT_LIMIT);
    if limit == 0 || limit > MAX_LIMIT {
        return Err(QueryError::Limit(limit));
    }
    let parsed = parse_query(&q.q)?;
    check_available(&parsed, false)?;
    let all = select(records, FindingView::bare, &parsed, &RunScopes::new());
    let total = all.len();
    let limit = if q.all { total } else { limit };
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
            "q": { "type": "string", "description": "Findings query, e.g. `severity>=high tag:class:sqli -tag:false-positive -has:poc \"sql injection\"`. Tokens AND; `key:a,b` is any-of; `-` negates; keys: severity (>=,>,<=,<), tag, has (tags|report|poc|cwe), cwe, owner, product, verified, profile, scope, concern, agent, file (path prefix), run, id; bare words search title/summary/id/file. Empty matches everything." },
            "limit": { "type": "integer", "minimum": 1, "maximum": MAX_LIMIT, "description": "Rows per page (default 50)." },
            "cursor": { "type": "string", "description": "`next_cursor` from the previous page." },
            "all": { "type": "boolean", "description": "Return every match in one answer, unpaged (no `limit`/`cursor`) and unbounded. For a workflow `for_each` over the rows; to read findings into a conversation, page instead." }
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

    #[test]
    fn an_empty_query_pages_every_finding_in_severity_order() {
        let f = fixture();
        let p = query(&f, &q()).unwrap();
        let ids: Vec<&str> = p.rows.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, ["fnd_crit", "fnd_high_new", "fnd_high_old", "fnd_low"]);
    }

    #[test]
    fn query_parses_q_and_pages() {
        let f = fixture();
        let p = query(
            &f,
            &FindingQuery {
                q: "tag:needs-poc".into(),
                ..q()
            },
        )
        .unwrap();
        assert_eq!(p.total, 2);
        let p = query(
            &f,
            &FindingQuery {
                q: "severity>=high".into(),
                limit: Some(1),
                ..q()
            },
        )
        .unwrap();
        assert_eq!(p.rows.len(), 1);
        assert_eq!(p.total, 3);
        assert!(p.next_cursor.is_some());
    }

    #[test]
    fn query_refuses_bad_q_and_provenance_keys() {
        let f = fixture();
        assert!(matches!(
            query(
                &f,
                &FindingQuery {
                    q: "sevrity:x".into(),
                    ..q()
                }
            ),
            Err(QueryError::Parse(_))
        ));
        assert!(matches!(
            query(
                &f,
                &FindingQuery {
                    q: "project:x".into(),
                    ..q()
                }
            ),
            Err(QueryError::Unavailable(_))
        ));
    }

    #[test]
    fn query_input_is_q_limit_cursor_all_only() {
        let ok: FindingQuery =
            serde_json::from_value(serde_json::json!({"q": "tag:x", "limit": 5})).unwrap();
        assert_eq!(ok.q, "tag:x");
        assert!(
            serde_json::from_value::<FindingQuery>(serde_json::json!({"tags": ["x"]})).is_err()
        );
        let empty: FindingQuery = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(empty.q, "");
        assert!(!empty.all);
        let all: FindingQuery = serde_json::from_value(serde_json::json!({"all": true})).unwrap();
        assert!(all.all);
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
    fn all_returns_every_match_past_the_page_cap() {
        let f: Vec<FindingRecord> = (0..MAX_LIMIT as u32 + 100)
            .map(|i| {
                rec(
                    &format!("fnd_{i:04}"),
                    Severity::High,
                    i % 60,
                    &["needs-poc"],
                )
            })
            .chain([rec("fnd_other", Severity::High, 0, &[])])
            .collect();
        let p = query(
            &f,
            &FindingQuery {
                q: "tag:needs-poc".into(),
                all: true,
                ..q()
            },
        )
        .unwrap();
        assert_eq!(p.total, MAX_LIMIT + 100);
        assert_eq!(p.rows.len(), MAX_LIMIT + 100);
        assert_eq!(p.next_cursor, None);
        let none = query(
            &f,
            &FindingQuery {
                q: "tag:nothing".into(),
                all: true,
                ..q()
            },
        )
        .unwrap();
        assert_eq!(
            (none.total, none.rows.len(), none.next_cursor),
            (0, 0, None)
        );
    }

    #[test]
    fn all_refuses_limit_and_cursor() {
        let f = fixture();
        for paged in [
            FindingQuery {
                all: true,
                limit: Some(10),
                ..q()
            },
            FindingQuery {
                all: true,
                cursor: Some("4|2026-01-01T00:00:00Z|fnd_x".into()),
                ..q()
            },
        ] {
            assert_eq!(query(&f, &paged).unwrap_err(), QueryError::AllWithPaging);
        }
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
}
