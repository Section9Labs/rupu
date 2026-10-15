//! `?q=` on `GET /api/runs/autoflows/runs`: every workflow-run field
//! ([`super::workflow_runs`]' own consts, in its order) over the autoflow
//! run rows and attempt rows, plus the fields only those rows have — the
//! issue, the PR, the row's kind and an attempt's error.
//!
//! The `autoflow` key is read the way `customer` is: a row WITHOUT it comes
//! from a host that can't report it (`None`, so the host is unfilterable),
//! `null` is a manual run (no values) and an object is an autoflow's.

use std::borrow::Cow;

use rupu_query::{FieldDef, FieldKind, MatchMode, RowField, Unreportable};
use serde_json::Value;

use super::shared::{CODENAME, CUSTOMER, HOST};
use super::workflow_runs::{ID, IS, SINCE, STATUS, TRIGGER, UNTIL, WORKFLOW};
use super::Fields;

/// The row's `autoflow`: `Err` when the row carries no key, `Ok(None)` for
/// `null`, `Ok(Some(_))` for an autoflow's object.
fn autoflow(row: &Value) -> Result<Option<&Value>, Unreportable> {
    match row.get("autoflow") {
        None => Err(Unreportable { field: "autoflow" }),
        Some(Value::Null) => Ok(None),
        Some(v) => Ok(Some(v)),
    }
}

/// Whether `row` is an autoflow's (a run an autoflow launched, or an
/// attempt): what the list keeps ([`crate::query::RowFilter::only`]).
pub fn is_autoflow_row(row: &Value) -> Result<bool, Unreportable> {
    autoflow(row).map(|af| af.is_some())
}

fn issue(row: &Value) -> Option<Vec<Cow<'_, str>>> {
    let af = autoflow(row).ok()?;
    Some(
        af.into_iter()
            .flat_map(|a| {
                ["issue_display_ref", "issue_ref"]
                    .into_iter()
                    .filter_map(move |k| a.get(k).and_then(Value::as_str))
            })
            .filter(|s| !s.is_empty())
            .map(Cow::Borrowed)
            .collect(),
    )
}

fn pr(row: &Value) -> Option<Vec<Cow<'_, str>>> {
    let af = autoflow(row).ok()?;
    let has = af
        .and_then(|a| a.get("pr_url"))
        .and_then(Value::as_str)
        .is_some_and(|s| !s.is_empty());
    Some(vec![Cow::Borrowed(if has { "any" } else { "none" })])
}

fn kind(row: &Value) -> Option<Vec<Cow<'_, str>>> {
    let attempt = row.get("attempt").is_some_and(Value::is_object);
    Some(vec![Cow::Borrowed(if attempt { "attempt" } else { "run" })])
}

fn error(row: &Value) -> Option<Vec<Cow<'_, str>>> {
    Some(
        row.get("attempt")
            .and_then(|a| a.get("detail"))
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(|s| vec![Cow::Borrowed(s)])
            .unwrap_or_default(),
    )
}

pub static FIELDS: Fields = &[
    STATUS,
    WORKFLOW,
    ID,
    CODENAME,
    TRIGGER,
    HOST,
    CUSTOMER,
    SINCE,
    UNTIL,
    IS,
    RowField {
        def: FieldDef::new("issue", FieldKind::Text),
        get: issue,
        mode: MatchMode::Contains,
        text: true,
        facet: false,
    },
    RowField {
        def: FieldDef::one_of("pr", &["any", "none"]),
        get: pr,
        mode: MatchMode::Exact,
        text: false,
        facet: true,
    },
    RowField {
        def: FieldDef::one_of("kind", &["run", "attempt"]),
        get: kind,
        mode: MatchMode::Exact,
        text: false,
        facet: true,
    },
    RowField {
        def: FieldDef::new("error", FieldKind::Text),
        get: error,
        mode: MatchMode::Contains,
        text: true,
        facet: false,
    },
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::{workflow_runs, ListQuery};
    use serde_json::json;

    /// The autoflow registry starts with every workflow-run field, the
    /// very same entries (def, match and getter), in the same order.
    #[test]
    fn starts_with_every_workflow_run_field() {
        assert!(FIELDS.len() > workflow_runs::FIELDS.len());
        for (w, a) in workflow_runs::FIELDS.iter().zip(FIELDS) {
            assert_eq!(
                serde_json::to_value(w).unwrap(),
                serde_json::to_value(a).unwrap()
            );
            assert_eq!(w.get as usize, a.get as usize, "{}: getter", w.def.key);
        }
    }

    fn q(s: &str) -> ListQuery {
        ListQuery::parse(Some(s), FIELDS).unwrap().unwrap()
    }

    #[test]
    fn a_row_without_the_autoflow_key_cannot_report_issue_or_pr() {
        let row = json!({"id": "r1", "status": "failed"});
        // `matches` names the term's key (as `customer:` does), the list's
        // own predicate the row key.
        for (query, field) in [("issue:x", "issue"), ("pr:any", "pr"), ("-pr:none", "pr")] {
            assert_eq!(
                q(query).matches(&row),
                Err(Unreportable { field }),
                "{query}"
            );
        }
        // `kind` and `error` read the row itself: an older host reports them.
        assert_eq!(q("kind:run").matches(&row), Ok(true));
        assert_eq!(q("error:boom").matches(&row), Ok(false));
        assert_eq!(
            is_autoflow_row(&row),
            Err(Unreportable { field: "autoflow" })
        );
    }

    #[test]
    fn a_manual_run_has_no_issue_and_no_pr() {
        let row = json!({"id": "r1", "autoflow": null});
        assert_eq!(q("issue:x").matches(&row), Ok(false));
        assert_eq!(q("pr:none").matches(&row), Ok(true));
        assert_eq!(is_autoflow_row(&row), Ok(false));
    }

    #[test]
    fn issue_reads_the_display_ref_and_the_ref() {
        let row = json!({"id": "r1", "autoflow": {
            "issue_ref": "github:acme/api/issues/7",
            "issue_display_ref": "acme/api#7",
            "pr_url": "https://example.test/pr/9",
        }});
        assert_eq!(q("issue:api#7").matches(&row), Ok(true));
        assert_eq!(q("issue:issues/7").matches(&row), Ok(true));
        assert_eq!(q("issue:widgets").matches(&row), Ok(false));
        // A bare word searches the issue too.
        assert_eq!(q("ACME/API").matches(&row), Ok(true));
        assert_eq!(q("pr:any").matches(&row), Ok(true));
        assert_eq!(q("pr:none").matches(&row), Ok(false));
        assert_eq!(is_autoflow_row(&row), Ok(true));
    }

    #[test]
    fn kind_and_error_read_the_attempt() {
        let run = json!({"id": "r1", "autoflow": {"issue_ref": "x"}});
        let attempt = json!({"id": "afe_1", "autoflow": {"issue_ref": "x"},
            "attempt": {"detail": "dispatch failed: branch exists", "cycle_id": "c"}});
        assert_eq!(q("kind:run").matches(&run), Ok(true));
        assert_eq!(q("kind:attempt").matches(&run), Ok(false));
        assert_eq!(q("kind:attempt").matches(&attempt), Ok(true));
        assert_eq!(q("error:\"branch exists\"").matches(&attempt), Ok(true));
        assert_eq!(q("error:\"branch exists\"").matches(&run), Ok(false));
        assert_eq!(q("branch").matches(&attempt), Ok(true));
        let facets = rupu_query::facets(&[run, attempt], FIELDS);
        assert_eq!(facets["kind"]["run"], 1);
        assert_eq!(facets["kind"]["attempt"], 1);
        assert_eq!(facets["pr"]["none"], 2);
    }
}
