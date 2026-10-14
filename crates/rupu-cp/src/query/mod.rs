//! `?q=` on the paged lists: one [`rupu_query::RowField`] registry per list
//! over its wire rows (JSON, so a local row and a remote host's row are read
//! the same way), and the [`RowFilter`] every list applies before it pages —
//! the `?customer=` filter and the query together. Each registry is pinned
//! by `tests/fixtures/list_query/<list>.fields.json`, which the web's twin
//! registries are checked against too.

pub mod agent_runs;
pub mod sessions;
pub mod workflow_runs;

use std::borrow::Cow;
use std::sync::Arc;

use axum::response::{IntoResponse, Response};
use axum::Json;
use chrono::{DateTime, Utc};
use rupu_query::{Facets, ParseError, ParsedQuery, RowField, Unreportable};
use serde_json::Value;

use crate::error::ApiError;

/// A list's registry.
pub type Fields = &'static [RowField<Value>];

/// The response header naming the hosts left out of a list because their
/// rows can't report a field the query names (other than `customer`, which
/// keeps [`crate::customers::HOSTS_WITHOUT_CUSTOMER_HEADER`]).
pub const HOSTS_UNFILTERABLE_HEADER: &str = "x-rupu-hosts-unfilterable";

/// A parsed `?q=`, with its registry and the `now` its spans resolve
/// against.
pub struct ListQuery {
    pub parsed: ParsedQuery,
    fields: Fields,
    now: DateTime<Utc>,
}

impl ListQuery {
    /// `None` for an absent or blank `q`.
    pub fn parse(q: Option<&str>, fields: Fields) -> Result<Option<Self>, ParseError> {
        let Some(q) = q else { return Ok(None) };
        let parsed = rupu_query::parse_query(q, &rupu_query::defs(fields))?;
        if parsed.terms.is_empty() {
            return Ok(None);
        }
        Ok(Some(Self {
            parsed,
            fields,
            now: Utc::now(),
        }))
    }

    pub fn matches(&self, row: &Value) -> Result<bool, Unreportable> {
        rupu_query::matches(row, &self.parsed, self.fields, self.now)
    }

    /// Whether any term, negated or not, reads `key`.
    pub fn reads(&self, key: &str) -> bool {
        self.parsed.terms_on(key).next().is_some()
    }

    /// Whether a non-negated term on `key` asks for `value`.
    pub fn asks_for(&self, key: &str, value: &str) -> bool {
        self.parsed
            .terms_on(key)
            .any(|t| !t.negated && t.values.iter().any(|v| v == value))
    }
}

/// What a list keeps before it pages: the `?customer=` filter AND the query.
#[derive(Clone, Default)]
pub struct RowFilter {
    pub customer: Option<crate::customers::CustomerFilter>,
    pub query: Option<Arc<ListQuery>>,
}

impl RowFilter {
    pub fn new(
        customer: Option<crate::customers::CustomerFilter>,
        query: Option<ListQuery>,
    ) -> Self {
        Self {
            customer,
            query: query.map(Arc::new),
        }
    }

    pub fn is_active(&self) -> bool {
        self.customer.is_some() || self.query.is_some()
    }

    /// Whether the filter reads a row's customer — `?customer=` or a
    /// `customer:` term — so local rows must be attributed fail-closed (an
    /// unreadable assignment is a 500, never "no customer").
    pub fn reads_customer(&self) -> bool {
        self.customer.is_some() || self.query.as_ref().is_some_and(|q| q.reads("customer"))
    }

    /// Whether `row` is in the list's scope: its `customer` key matches the
    /// customer filter. A row without the key (or a key that is neither a
    /// slug nor `null`) can't say.
    pub fn in_scope(&self, row: &Value) -> Result<bool, Unreportable> {
        let Some(f) = &self.customer else {
            return Ok(true);
        };
        let customer =
            crate::customers::row_customer(row).ok_or(Unreportable { field: "customer" })?;
        Ok(f.matches(customer))
    }

    /// Whether an in-scope `row` matches the query.
    pub fn matches_query(&self, row: &Value) -> Result<bool, Unreportable> {
        match &self.query {
            None => Ok(true),
            Some(q) => q.matches(row),
        }
    }

    /// Whether `row` is kept: in scope and matching the query.
    pub fn keeps(&self, row: &Value) -> Result<bool, Unreportable> {
        Ok(self.in_scope(row)? && self.matches_query(row)?)
    }

    /// The rows kept, in order; `Err` names the field the first unreadable
    /// row can't report (the host can't be filtered).
    pub fn retain(&self, rows: Vec<Value>) -> Result<Vec<Value>, Unreportable> {
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            if self.keeps(&row)? {
                out.push(row);
            }
        }
        Ok(out)
    }

    /// [`Self::retain`], also returning the in-scope rows (before the query)
    /// when `scoped` is asked for — what facets count.
    pub fn split(
        &self,
        rows: Vec<Value>,
        scoped: bool,
    ) -> Result<(Vec<Value>, Option<Vec<Value>>), Unreportable> {
        let mut kept = Vec::new();
        let mut in_scope = scoped.then(Vec::new);
        for row in rows {
            if !self.in_scope(&row)? {
                continue;
            }
            let hit = self.matches_query(&row)?;
            match (&mut in_scope, hit) {
                (Some(s), true) => {
                    kept.push(row.clone());
                    s.push(row);
                }
                (Some(s), false) => s.push(row),
                (None, true) => kept.push(row),
                (None, false) => {}
            }
        }
        Ok((kept, in_scope))
    }
}

/// A host that couldn't be filtered: its id and the field its rows lack.
pub type Unfilterable = (String, &'static str);

/// What [`filter_merged`] kept.
#[derive(Debug, Default)]
pub struct Merged {
    pub rows: Vec<Value>,
    /// Every in-scope row before the query, when asked for (facets).
    pub scoped: Vec<Value>,
    pub unfilterable: Vec<Unfilterable>,
}

impl Merged {
    /// The response headers naming the hosts left out
    /// ([`unfilterable_headers`]).
    pub fn headers(&self) -> axum::http::HeaderMap {
        unfilterable_headers(&self.unfilterable)
    }
}

/// Filter a merged, host-tagged (`host_id`) list: a host any of whose rows
/// can't report a field `filter` reads is left out whole and named; every
/// other row is kept when `scope` (the list's own params: lifecycle, dates,
/// scope) and `filter` keep it. Order is preserved.
pub fn filter_merged(
    rows: Vec<Value>,
    filter: &RowFilter,
    scope: impl Fn(&Value) -> bool,
    facets: bool,
) -> Merged {
    let host_of = |row: &Value| row["host_id"].as_str().unwrap_or("local").to_string();
    let mut out = Merged::default();
    for row in &rows {
        let read = filter.in_scope(row).and_then(|_| filter.matches_query(row));
        if let Err(e) = read {
            note_unfilterable(&mut out.unfilterable, &host_of(row), e.field);
        }
    }
    for row in rows {
        let host = host_of(&row);
        if out.unfilterable.iter().any(|(h, _)| *h == host) {
            continue;
        }
        if !scope(&row) || filter.in_scope(&row) != Ok(true) {
            continue;
        }
        let hit = filter.matches_query(&row) == Ok(true);
        if facets {
            out.scoped.push(row.clone());
        }
        if hit {
            out.rows.push(row);
        }
    }
    out
}

/// The headers naming the hosts a fan-out left out: hosts that can't report
/// `customer` in [`crate::customers::HOSTS_WITHOUT_CUSTOMER_HEADER`] (as
/// `?customer=` always has), any other field in
/// [`HOSTS_UNFILTERABLE_HEADER`] as `host:field`.
pub fn unfilterable_headers(hosts: &[Unfilterable]) -> axum::http::HeaderMap {
    let without_customer: Vec<String> = hosts
        .iter()
        .filter(|(_, f)| *f == "customer")
        .map(|(h, _)| h.clone())
        .collect();
    let mut headers = crate::customers::hosts_without_customer_header(&without_customer);
    let other: Vec<String> = hosts
        .iter()
        .filter(|(_, f)| *f != "customer")
        .map(|(h, f)| format!("{h}:{f}"))
        .collect();
    if !other.is_empty() {
        if let Ok(v) = axum::http::HeaderValue::from_str(&other.join(",")) {
            headers.insert(HOSTS_UNFILTERABLE_HEADER, v);
        }
    }
    headers
}

/// The 501 a single-host list answers when the host's rows can't report a
/// field the request filters on.
pub fn unfilterable_error(host_id: &str, field: &str) -> ApiError {
    if field == "customer" {
        return crate::customers::customers_unsupported(host_id);
    }
    ApiError::not_available(format!(
        "host {host_id} can't report `{field}` for every row"
    ))
}

/// Push `(host, field)` once.
pub fn note_unfilterable(hosts: &mut Vec<Unfilterable>, host: &str, field: &'static str) {
    if !hosts.iter().any(|(h, f)| h == host && *f == field) {
        hosts.push((host.to_string(), field));
    }
}

/// A list handler's failure: an [`ApiError`], or a `q` that doesn't parse —
/// a 400 `{error, token, code, start, end}`, as `GET /api/findings?q=`.
#[derive(Debug)]
pub enum ListError {
    Api(ApiError),
    Query(ParseError),
}

impl From<ApiError> for ListError {
    fn from(e: ApiError) -> Self {
        Self::Api(e)
    }
}

impl From<ParseError> for ListError {
    fn from(e: ParseError) -> Self {
        Self::Query(e)
    }
}

impl IntoResponse for ListError {
    fn into_response(self) -> Response {
        match self {
            Self::Api(e) => e.into_response(),
            Self::Query(e) => (
                axum::http::StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": e.message,
                    "token": e.token,
                    "code": e.code,
                    "start": e.start,
                    "end": e.end,
                })),
            )
                .into_response(),
        }
    }
}

/// A paged list's response: its rows (a JSON array, as ever), or — with
/// `?facets=1` — `{rows, facets, facets_partial}`.
#[derive(Debug)]
pub struct ListResponse {
    pub headers: axum::http::HeaderMap,
    pub rows: Vec<Value>,
    pub facets: Option<FacetCounts>,
}

/// Value counts of the list's facet fields over its scope before `q`.
/// `partial`: some remote host's rows were read one page only, so its
/// counts cover that page.
#[derive(Debug)]
pub struct FacetCounts {
    pub facets: Facets,
    pub partial: bool,
}

impl IntoResponse for ListResponse {
    fn into_response(self) -> Response {
        match self.facets {
            None => (self.headers, Json(self.rows)).into_response(),
            Some(f) => (
                self.headers,
                Json(serde_json::json!({
                    "rows": self.rows,
                    "facets": f.facets,
                    "facets_partial": f.partial,
                })),
            )
                .into_response(),
        }
    }
}

/// The facets of `scoped` rows on `fields`.
pub fn facet_counts(scoped: &[Value], fields: Fields, partial: bool) -> FacetCounts {
    FacetCounts {
        facets: rupu_query::facets(scoped, fields),
        partial,
    }
}

/// `?facets=1` (or `true`).
pub fn wants_facets(raw: Option<&str>) -> bool {
    matches!(raw, Some("1" | "true"))
}

// ── getters shared by the registries ───────────────────────────────────

/// A string key: absent or `null` is no value; a non-string is its JSON
/// text (a status written as an object reads as its tag).
pub(crate) fn opt<'a>(row: &'a Value, key: &str) -> Option<Vec<Cow<'a, str>>> {
    Some(match row.get(key) {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::String(s)) if s.is_empty() => Vec::new(),
        Some(Value::String(s)) => vec![Cow::Borrowed(s.as_str())],
        Some(Value::Object(o)) => o.keys().map(|k| Cow::Borrowed(k.as_str())).collect(),
        Some(other) => vec![Cow::Owned(other.to_string())],
    })
}

/// The row's `customer`: a slug, `none` for `null`; a row without the key
/// can't report it.
pub(crate) fn customer(row: &Value) -> Option<Vec<Cow<'_, str>>> {
    let slug = crate::customers::row_customer(row)?.unwrap_or("none");
    Some(vec![Cow::Borrowed(slug)])
}

/// Field defs every list shares.
pub(crate) mod shared {
    use super::*;
    use rupu_query::{FieldDef, FieldKind, MatchMode};

    pub(crate) const HOST: RowField<Value> = RowField {
        def: FieldDef::new("host", FieldKind::Text),
        get: |r| opt(r, "host_id"),
        mode: MatchMode::Exact,
        text: false,
        facet: true,
    };

    pub(crate) const CUSTOMER: RowField<Value> = RowField {
        def: FieldDef::new("customer", FieldKind::Text),
        get: super::customer,
        mode: MatchMode::Exact,
        text: false,
        facet: true,
    };

    pub(crate) const CODENAME: RowField<Value> = RowField {
        def: FieldDef::new("codename", FieldKind::Text),
        get: |r| opt(r, "codename"),
        mode: MatchMode::Contains,
        text: true,
        facet: false,
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bad_query_is_a_400_with_its_position() {
        let err = ListQuery::parse(Some("status:done sevrity:x"), workflow_runs::FIELDS)
            .err()
            .unwrap();
        let resp = ListError::Query(err).into_response();
        assert_eq!(resp.status(), axum::http::StatusCode::BAD_REQUEST);
    }

    #[test]
    fn a_blank_query_is_no_query() {
        assert!(ListQuery::parse(Some("   "), workflow_runs::FIELDS)
            .unwrap()
            .is_none());
        assert!(ListQuery::parse(None, workflow_runs::FIELDS)
            .unwrap()
            .is_none());
    }

    #[test]
    fn scope_and_query_split() {
        let f = RowFilter::new(
            Some(crate::customers::CustomerFilter::Slug("acme".into())),
            ListQuery::parse(Some("status:failed"), workflow_runs::FIELDS).unwrap(),
        );
        let rows = vec![
            serde_json::json!({"id": "a", "status": "failed", "customer": "acme"}),
            serde_json::json!({"id": "b", "status": "completed", "customer": "acme"}),
            serde_json::json!({"id": "c", "status": "failed", "customer": null}),
        ];
        let (kept, scoped) = f.split(rows.clone(), true).unwrap();
        let ids = |v: &[Value]| {
            v.iter()
                .map(|r| r["id"].as_str().unwrap().to_string())
                .collect::<Vec<_>>()
        };
        assert_eq!(ids(&kept), ["a"]);
        assert_eq!(ids(&scoped.unwrap()), ["a", "b"]);
        let no_key = vec![serde_json::json!({"id": "d", "status": "failed"})];
        assert_eq!(f.retain(no_key), Err(Unreportable { field: "customer" }));
    }

    #[test]
    fn a_merged_list_names_the_hosts_it_left_out_in_its_headers() {
        let f = RowFilter::new(
            None,
            ListQuery::parse(Some("customer:acme"), workflow_runs::FIELDS).unwrap(),
        );
        let rows = vec![
            serde_json::json!({"id": "a", "host_id": "local", "customer": "acme"}),
            serde_json::json!({"id": "b", "host_id": "old"}),
        ];
        let merged = filter_merged(rows, &f, |_| true, false);
        assert_eq!(merged.rows.len(), 1);
        assert_eq!(
            merged.headers().get("x-rupu-hosts-without-customer").unwrap(),
            "old"
        );
    }

    #[test]
    fn unfilterable_hosts_split_across_the_two_headers() {
        let h = unfilterable_headers(&[
            ("old".into(), "customer"),
            ("ssh".into(), "is"),
            ("x".into(), "status"),
        ]);
        assert_eq!(h.get("x-rupu-hosts-without-customer").unwrap(), "old");
        assert_eq!(h.get(HOSTS_UNFILTERABLE_HEADER).unwrap(), "ssh:is,x:status");
    }
}
