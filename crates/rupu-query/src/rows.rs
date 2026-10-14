//! Evaluate a parsed query against rows through a registry of
//! [`RowField`]s: how each field reads a row and how its values match. A
//! term's values OR, terms AND, `-` negates, a bare word searches the fields
//! marked `text`; every comparison is case-insensitive. The web's
//! `src/lib/query/rows.ts` evaluates the same registries the same way.

use std::borrow::Cow;
use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::ser::SerializeStruct;
use serde::{Serialize, Serializer};

use crate::grammar::{FieldDef, Op, ParsedQuery, Term, TEXT_KEY};
use crate::time::{parse_timestamp, TimeValue};

/// How a term's value matches a row's value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum MatchMode {
    /// Equal.
    Exact,
    /// The row's value contains the term's.
    Contains,
    /// The row's value starts with the term's.
    Prefix,
    /// Compared by rank on the field's `values` ladder (`>=` …).
    Ordered,
    /// The row's timestamp is at or after the term's time.
    Since,
    /// The row's timestamp is at or before the term's time.
    Until,
}

/// A field's values on one row: `None` when the row cannot report the
/// field at all (a peer too old to carry the key), which is never read as
/// "no value".
pub type Getter<R> = for<'a> fn(&'a R) -> Option<Vec<Cow<'a, str>>>;

/// One entry of a list's registry: the grammar's field spec plus how it
/// reads and matches a row.
pub struct RowField<R: 'static> {
    pub def: FieldDef,
    pub get: Getter<R>,
    pub mode: MatchMode,
    /// Bare words search this field.
    pub text: bool,
    /// Suggest the values in use, with counts ([`facets`]).
    pub facet: bool,
}

impl<R> Serialize for RowField<R> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut st = s.serialize_struct("RowField", 7)?;
        st.serialize_field("key", self.def.key)?;
        st.serialize_field("aliases", self.def.aliases)?;
        st.serialize_field("kind", &self.def.kind)?;
        st.serialize_field("values", self.def.values)?;
        st.serialize_field("match", &self.mode)?;
        st.serialize_field("text", &self.text)?;
        st.serialize_field("facet", &self.facet)?;
        st.end()
    }
}

/// The grammar specs of a registry, for [`crate::parse_query`].
pub fn defs<R>(fields: &[RowField<R>]) -> Vec<FieldDef> {
    fields.iter().map(|f| f.def).collect()
}

/// A query names a field this row cannot report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Unreportable {
    pub field: &'static str,
}

/// Whether `row` matches every term of `q`, spans resolved against `now`.
/// Every keyed term is read, so a row that cannot report any field the query
/// names is `Err`, whatever the other terms say. A free-text term skips the
/// text fields the row can't report.
pub fn matches<R>(
    row: &R,
    q: &ParsedQuery,
    fields: &[RowField<R>],
    now: DateTime<Utc>,
) -> Result<bool, Unreportable> {
    let mut all = true;
    for term in &q.terms {
        all &= term_matches(row, term, fields, now)?;
    }
    Ok(all)
}

fn term_matches<R>(
    row: &R,
    term: &Term,
    fields: &[RowField<R>],
    now: DateTime<Utc>,
) -> Result<bool, Unreportable> {
    let hit = if term.key == TEXT_KEY {
        fields
            .iter()
            .filter(|f| f.text)
            .filter_map(|f| (f.get)(row))
            .flatten()
            .any(|v| {
                term.values
                    .iter()
                    .any(|w| v.to_lowercase().contains(&w.to_lowercase()))
            })
    } else {
        let unreportable = Unreportable { field: term.key };
        let f = fields
            .iter()
            .find(|f| f.def.key == term.key)
            .ok_or(unreportable)?;
        let values = (f.get)(row).ok_or(unreportable)?;
        term.values.iter().any(|want| {
            values
                .iter()
                .any(|v| value_matches(f, term.op, v, want, now))
        })
    };
    Ok(hit != term.negated)
}

fn value_matches<R>(f: &RowField<R>, op: Op, v: &str, want: &str, now: DateTime<Utc>) -> bool {
    match f.mode {
        MatchMode::Exact => v.to_lowercase() == want.to_lowercase(),
        MatchMode::Contains => v.to_lowercase().contains(&want.to_lowercase()),
        MatchMode::Prefix => v.to_lowercase().starts_with(&want.to_lowercase()),
        MatchMode::Ordered => {
            let rank = |s: &str| f.def.values.iter().position(|x| x.eq_ignore_ascii_case(s));
            let (Some(have), Some(want)) = (rank(v), rank(want)) else {
                return false;
            };
            match op {
                Op::Eq => have == want,
                Op::Gt => have > want,
                Op::Ge => have >= want,
                Op::Lt => have < want,
                Op::Le => have <= want,
            }
        }
        MatchMode::Since | MatchMode::Until => {
            let (Some(ts), Some(bound)) = (parse_timestamp(v), TimeValue::parse(want)) else {
                return false;
            };
            if f.mode == MatchMode::Since {
                bound.admits_since(ts, now)
            } else {
                bound.admits_until(ts, now)
            }
        }
    }
}

/// Value counts per facet field: `field key → value → rows carrying it`.
/// Every facet field has an entry, empty when no row carries a value.
pub type Facets = BTreeMap<&'static str, BTreeMap<String, usize>>;

/// The [`Facets`] of `rows` (a row counts once per distinct value; a row
/// that can't report a field adds nothing to it).
pub fn facets<'r, R: 'r>(rows: impl IntoIterator<Item = &'r R>, fields: &[RowField<R>]) -> Facets {
    let facet_fields: Vec<&RowField<R>> = fields.iter().filter(|f| f.facet).collect();
    let mut out: Facets = facet_fields
        .iter()
        .map(|f| (f.def.key, BTreeMap::new()))
        .collect();
    for row in rows {
        for f in &facet_fields {
            let Some(mut values) = (f.get)(row) else {
                continue;
            };
            values.sort();
            values.dedup();
            let counts = out.get_mut(f.def.key).expect("seeded above");
            for v in values {
                *counts.entry(v.into_owned()).or_default() += 1;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grammar::{parse_query, FieldKind};

    struct Run {
        name: &'static str,
        status: &'static str,
        level: &'static str,
        tags: &'static [&'static str],
        started: &'static str,
        owner: Option<&'static str>,
    }

    fn one(s: &str) -> Option<Vec<Cow<'_, str>>> {
        Some(vec![Cow::Borrowed(s)])
    }

    const FIELDS: &[RowField<Run>] = &[
        RowField {
            def: FieldDef::new("name", FieldKind::Text),
            get: |r| one(r.name),
            mode: MatchMode::Contains,
            text: true,
            facet: false,
        },
        RowField {
            def: FieldDef::one_of("status", &["running", "done", "failed"]),
            get: |r| one(r.status),
            mode: MatchMode::Exact,
            text: false,
            facet: true,
        },
        RowField {
            def: FieldDef {
                key: "level",
                aliases: &[],
                kind: FieldKind::Severity,
                values: &["low", "medium", "high"],
            },
            get: |r| one(r.level),
            mode: MatchMode::Ordered,
            text: false,
            facet: false,
        },
        RowField {
            def: FieldDef::new("tag", FieldKind::Text),
            get: |r| Some(r.tags.iter().map(|t| Cow::Borrowed(*t)).collect()),
            mode: MatchMode::Prefix,
            text: false,
            facet: true,
        },
        RowField {
            def: FieldDef::new("since", FieldKind::Time),
            get: |r| one(r.started),
            mode: MatchMode::Since,
            text: false,
            facet: false,
        },
        RowField {
            def: FieldDef::new("until", FieldKind::Time),
            get: |r| one(r.started),
            mode: MatchMode::Until,
            text: false,
            facet: false,
        },
        RowField {
            def: FieldDef::new("owner", FieldKind::Text),
            get: |r| r.owner.map(|o| vec![Cow::Borrowed(o)]),
            mode: MatchMode::Exact,
            text: true,
            facet: true,
        },
    ];

    const RUNS: &[Run] = &[
        Run {
            name: "Nightly-Review",
            status: "done",
            level: "high",
            tags: &["web", "prod"],
            started: "2026-10-14T06:00:00Z",
            owner: Some("ana"),
        },
        Run {
            name: "deploy",
            status: "failed",
            level: "low",
            tags: &["prod-eu"],
            started: "2026-10-01T09:00:00Z",
            owner: Some("bo"),
        },
        Run {
            name: "review-bot",
            status: "running",
            level: "medium",
            tags: &[],
            started: "garbage",
            owner: None,
        },
    ];

    fn now() -> DateTime<Utc> {
        parse_timestamp("2026-10-14T12:00:00Z").unwrap()
    }

    fn defs() -> Vec<FieldDef> {
        super::defs(FIELDS)
    }

    /// The names of the runs `q` keeps; `Err` names the first run whose
    /// field it can't read.
    fn hits(q: &str) -> Result<Vec<&'static str>, (&'static str, &'static str)> {
        let q = parse_query(q, &defs()).unwrap();
        let mut out = Vec::new();
        for r in RUNS {
            match matches(r, &q, FIELDS, now()) {
                Ok(true) => out.push(r.name),
                Ok(false) => {}
                Err(e) => return Err((r.name, e.field)),
            }
        }
        Ok(out)
    }

    #[test]
    fn values_or_and_terms_and() {
        assert_eq!(
            hits("status:done,failed"),
            Ok(vec!["Nightly-Review", "deploy"])
        );
        assert_eq!(hits("status:done,failed tag:prod-"), Ok(vec!["deploy"]));
    }

    #[test]
    fn negation_inverts_a_term() {
        assert_eq!(
            hits("-status:FAILED"),
            Ok(vec!["Nightly-Review", "review-bot"])
        );
    }

    #[test]
    fn bare_words_search_text_fields_case_insensitively() {
        assert_eq!(hits("REVIEW"), Ok(vec!["Nightly-Review", "review-bot"]));
        assert_eq!(hits("an"), Ok(vec!["Nightly-Review"]));
        assert_eq!(hits("-review"), Ok(vec!["deploy"]));
    }

    #[test]
    fn match_modes() {
        assert_eq!(hits("name:IGHTLY"), Ok(vec!["Nightly-Review"]));
        assert_eq!(hits("tag:pro"), Ok(vec!["Nightly-Review", "deploy"]));
        assert_eq!(hits("tag:eu"), Ok(vec![]));
        assert_eq!(
            hits("level>=medium"),
            Ok(vec!["Nightly-Review", "review-bot"])
        );
        assert_eq!(hits("level<medium"), Ok(vec!["deploy"]));
        assert_eq!(hits("level:low"), Ok(vec!["deploy"]));
    }

    #[test]
    fn time_bounds_resolve_against_now_and_skip_unparseable_timestamps() {
        assert_eq!(hits("since:1d"), Ok(vec!["Nightly-Review"]));
        assert_eq!(hits("until:1w"), Ok(vec!["deploy"]));
        assert_eq!(
            hits("since:2026-10-01 until:2026-10-01"),
            Ok(vec!["deploy"])
        );
        assert_eq!(
            hits("until:2026-10-14T06:00:00Z"),
            Ok(vec!["Nightly-Review", "deploy"])
        );
    }

    #[test]
    fn a_field_the_row_cannot_report_is_an_error_not_a_miss() {
        assert_eq!(hits("owner:ana"), Err(("review-bot", "owner")));
        assert_eq!(hits("-owner:ana"), Err(("review-bot", "owner")));
        assert_eq!(hits("status:done owner:ana"), Err(("review-bot", "owner")));
    }

    #[test]
    fn facets_count_rows_per_value() {
        let f = facets(RUNS, FIELDS);
        assert_eq!(
            serde_json::to_value(f).unwrap(),
            serde_json::json!({
                "status": {"done": 1, "failed": 1, "running": 1},
                "tag": {"prod": 1, "prod-eu": 1, "web": 1},
                "owner": {"ana": 1, "bo": 1},
            })
        );
    }

    #[test]
    fn a_row_field_serializes_its_grammar_and_semantics() {
        assert_eq!(
            serde_json::to_value(&FIELDS[1]).unwrap(),
            serde_json::json!({
                "key": "status", "aliases": [], "kind": "enum",
                "values": ["running", "done", "failed"],
                "match": "exact", "text": false, "facet": true,
            })
        );
    }
}
