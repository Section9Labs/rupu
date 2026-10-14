//! The findings query language (spec "Query language"). One line, e.g.
//! `severity>=high tag:class:sqli -tag:false-positive "sql injection"`.
//!
//! The grammar is `rupu-query`'s; this module is the findings registry
//! ([`FIELDS`], with the `tag` and `cwe` kinds' normalizers) and the [`Key`]
//! each term's field maps to. `ledger::finding_filter` evaluates. The web's
//! `src/lib/findingQuery/grammar.ts` is the grammar's twin, held in lockstep
//! by `tests/fixtures/finding_query/`. Change both, and the fixtures,
//! together.

use crate::ledger::tags::Tag;
use crate::report::cwe::parse_cwe;
use serde::Serialize;

pub use rupu_query::{ErrorCode, FieldDef, FieldKind, Op, ParseError};

/// A field a term filters on; `Text` is free text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Key {
    Severity,
    Tag,
    Has,
    Project,
    Cwe,
    Owner,
    Product,
    Verified,
    Profile,
    Scope,
    Concern,
    Agent,
    Workflow,
    File,
    Run,
    Id,
    Text,
}

impl Key {
    /// The key a registry field (or free text) maps to.
    fn of(key: &str) -> Self {
        match key {
            "severity" => Self::Severity,
            "tag" => Self::Tag,
            "has" => Self::Has,
            "project" => Self::Project,
            "cwe" => Self::Cwe,
            "owner" => Self::Owner,
            "product" => Self::Product,
            "verified" => Self::Verified,
            "profile" => Self::Profile,
            "scope" => Self::Scope,
            "concern" => Self::Concern,
            "agent" => Self::Agent,
            "workflow" => Self::Workflow,
            "file" => Self::File,
            "run" => Self::Run,
            "id" => Self::Id,
            rupu_query::TEXT_KEY => Self::Text,
            other => unreachable!("`{other}` is not in the findings registry"),
        }
    }
}

/// One token of a query. `values` are OR-ed. A `Term` matches when any
/// value matches, inverted when `negated`. Values are normalized: enum and
/// severity values are lowercase, tags pass `Tag::parse`, CWEs are `CWE-<n>`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Term {
    #[serde(rename = "neg")]
    pub negated: bool,
    pub key: Key,
    pub op: Op,
    pub values: Vec<String>,
}

/// Every term must match (AND).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ParsedQuery {
    pub terms: Vec<Term>,
}

pub const SEVERITIES: &[&str] = &["info", "low", "medium", "high", "critical"];

fn tag(v: &str) -> Result<String, String> {
    Tag::parse(v).map(String::from).map_err(|e| e.to_string())
}

fn cwe(v: &str) -> Result<String, String> {
    parse_cwe(v)
        .map(|n| format!("CWE-{n}"))
        .ok_or_else(|| format!("`{v}` is not a CWE id (expected 79 or CWE-79)"))
}

const TEXT: FieldKind = FieldKind::Text;

/// The findings field registry, in suggestion order.
pub const FIELDS: &[FieldDef] = &[
    FieldDef {
        key: "severity",
        aliases: &["sev"],
        kind: FieldKind::Severity,
        values: SEVERITIES,
    },
    FieldDef::new(
        "tag",
        FieldKind::Custom {
            name: "tag",
            normalize: tag,
        },
    ),
    FieldDef::one_of("has", &["tags", "report", "poc", "cwe"]),
    FieldDef::new("project", TEXT),
    FieldDef::new(
        "cwe",
        FieldKind::Custom {
            name: "cwe",
            normalize: cwe,
        },
    ),
    FieldDef::new("owner", TEXT),
    FieldDef::new("product", TEXT),
    FieldDef::one_of(
        "verified",
        &["unverified", "confirmed", "disputed", "inconclusive"],
    ),
    FieldDef::one_of("profile", &["full", "summary"]),
    FieldDef::one_of(
        "scope",
        &["line", "file", "repo", "host", "endpoint", "resource"],
    ),
    FieldDef::new("concern", TEXT),
    FieldDef::new("agent", TEXT),
    FieldDef::new("workflow", TEXT),
    FieldDef::new("file", TEXT),
    FieldDef::new("run", TEXT),
    FieldDef::new("id", TEXT),
];

/// The field named `name` (or one of its aliases), case-insensitively.
pub fn field(name: &str) -> Option<&'static FieldDef> {
    rupu_query::field(FIELDS, name)
}

/// Parse a query. An empty or all-whitespace query has no terms and matches
/// everything.
pub fn parse_query(q: &str) -> Result<ParsedQuery, ParseError> {
    let parsed = rupu_query::parse_query(q, FIELDS)?;
    Ok(ParsedQuery {
        terms: parsed
            .terms
            .into_iter()
            .map(|t| Term {
                negated: t.negated,
                key: Key::of(t.key),
                op: t.op,
                values: t.values,
            })
            .collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_registry_field_has_a_key() {
        for f in FIELDS {
            assert_eq!(
                serde_json::to_value(Key::of(f.key)).unwrap(),
                serde_json::json!(f.key)
            );
        }
    }
}
