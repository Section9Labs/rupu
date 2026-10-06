//! The findings query language (spec "Query language"). One line, e.g.
//! `severity>=high tag:class:sqli -tag:false-positive "sql injection"`.
//!
//! This module only parses and validates. `ledger::finding_filter`
//! evaluates. The web's `src/lib/findingQuery/grammar.ts` is a twin of this
//! file, held in lockstep by `tests/fixtures/finding_query/`. Change both,
//! and the fixtures, together.

use crate::ledger::tags::Tag;
use crate::report::cwe::parse_cwe;
use serde::Serialize;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Op {
    Eq,
    Gt,
    Ge,
    Lt,
    Le,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    UnknownKey,
    EmptyValue,
    BadValue,
    BadOperator,
    UnclosedQuote,
    BadQuote,
}

/// Why a query does not parse: the 0-based token and its char span
/// (Unicode scalar values, `[start, end)`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, thiserror::Error)]
#[error("{message}")]
pub struct ParseError {
    pub token: usize,
    pub start: usize,
    pub end: usize,
    pub code: ErrorCode,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum FieldKind {
    Severity,
    Tag,
    Cwe,
    Enum,
    Text,
}

/// A queryable field. Serializes to the `fields.json` lockstep shape.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct FieldDef {
    pub key: &'static str,
    pub aliases: &'static [&'static str],
    pub kind: FieldKind,
    pub values: &'static [&'static str],
    #[serde(skip)]
    pub id: Key,
}

pub const SEVERITIES: &[&str] = &["info", "low", "medium", "high", "critical"];

const fn def(key: &'static str, id: Key, kind: FieldKind) -> FieldDef {
    FieldDef {
        key,
        aliases: &[],
        kind,
        values: &[],
        id,
    }
}

const fn enum_def(key: &'static str, id: Key, values: &'static [&'static str]) -> FieldDef {
    FieldDef {
        key,
        aliases: &[],
        kind: FieldKind::Enum,
        values,
        id,
    }
}

/// The findings field registry, in suggestion order.
pub const FIELDS: &[FieldDef] = &[
    FieldDef {
        key: "severity",
        aliases: &["sev"],
        kind: FieldKind::Severity,
        values: SEVERITIES,
        id: Key::Severity,
    },
    def("tag", Key::Tag, FieldKind::Tag),
    enum_def("has", Key::Has, &["tags", "report", "poc", "cwe"]),
    def("project", Key::Project, FieldKind::Text),
    def("cwe", Key::Cwe, FieldKind::Cwe),
    def("owner", Key::Owner, FieldKind::Text),
    def("product", Key::Product, FieldKind::Text),
    enum_def(
        "verified",
        Key::Verified,
        &["unverified", "confirmed", "disputed", "inconclusive"],
    ),
    enum_def("profile", Key::Profile, &["full", "summary"]),
    enum_def(
        "scope",
        Key::Scope,
        &["line", "file", "repo", "host", "endpoint", "resource"],
    ),
    def("concern", Key::Concern, FieldKind::Text),
    def("agent", Key::Agent, FieldKind::Text),
    def("workflow", Key::Workflow, FieldKind::Text),
    def("file", Key::File, FieldKind::Text),
    def("run", Key::Run, FieldKind::Text),
    def("id", Key::Id, FieldKind::Text),
];

/// The field named `name` (or one of its aliases), case-insensitively.
pub fn field(name: &str) -> Option<&'static FieldDef> {
    let n = name.to_ascii_lowercase();
    FIELDS
        .iter()
        .find(|f| f.key == n || f.aliases.contains(&n.as_str()))
}

struct RawToken {
    start: usize,
    end: usize,
    text: Vec<char>,
}

fn is_op_char(c: char) -> bool {
    matches!(c, ':' | '<' | '>' | '=')
}

fn err(
    token: usize,
    start: usize,
    end: usize,
    code: ErrorCode,
    message: impl Into<String>,
) -> ParseError {
    ParseError {
        token,
        start,
        end,
        code,
        message: message.into(),
    }
}

/// Split into tokens at unquoted whitespace. A quote opens only at an item
/// start: the token start (after an optional `-`), right after an operator
/// character, or right after a `,`. `\` escapes the next char, in or out of
/// quotes.
fn scan(q: &str) -> Result<Vec<RawToken>, ParseError> {
    let chars: Vec<char> = q.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i].is_whitespace() {
            i += 1;
            continue;
        }
        let start = i;
        let mut item_start = true;
        let mut quote: Option<char> = None;
        while i < chars.len() {
            let c = chars[i];
            if let Some(qc) = quote {
                if c == '\\' {
                    i += 2;
                    continue;
                }
                if c == qc {
                    quote = None;
                    item_start = false;
                }
                i += 1;
                continue;
            }
            if c.is_whitespace() {
                break;
            }
            if c == '\\' {
                i += 2;
                item_start = false;
                continue;
            }
            if item_start && (c == '"' || c == '\'') {
                quote = Some(c);
                i += 1;
                continue;
            }
            item_start = is_op_char(c) || c == ',' || (c == '-' && i == start);
            i += 1;
        }
        let end = i.min(chars.len());
        if quote.is_some() {
            return Err(err(
                out.len(),
                start,
                end,
                ErrorCode::UnclosedQuote,
                "this quote is never closed",
            ));
        }
        out.push(RawToken {
            start,
            end,
            text: chars[start..end].to_vec(),
        });
        i = end;
    }
    Ok(out)
}

/// Decode items from `s` (escapes resolved, quotes removed). With `split`,
/// an unescaped, unquoted `,` separates items.
fn items(s: &[char], split: bool) -> Result<Vec<String>, (ErrorCode, &'static str)> {
    let mut out = Vec::new();
    let mut i = 0;
    loop {
        let mut cur = String::new();
        if i < s.len() && (s[i] == '"' || s[i] == '\'') {
            let qc = s[i];
            i += 1;
            loop {
                if i >= s.len() {
                    return Err((ErrorCode::UnclosedQuote, "this quote is never closed"));
                }
                let c = s[i];
                if c == '\\' && i + 1 < s.len() {
                    cur.push(s[i + 1]);
                    i += 2;
                    continue;
                }
                i += 1;
                if c == qc {
                    break;
                }
                cur.push(c);
            }
            if i < s.len() && !(split && s[i] == ',') {
                return Err((
                    ErrorCode::BadQuote,
                    "put a space or a comma after a closing quote",
                ));
            }
        } else {
            while i < s.len() {
                let c = s[i];
                if c == '\\' && i + 1 < s.len() {
                    cur.push(s[i + 1]);
                    i += 2;
                    continue;
                }
                if split && c == ',' {
                    break;
                }
                cur.push(c);
                i += 1;
            }
        }
        if cur.is_empty() {
            return Err((ErrorCode::EmptyValue, "empty value"));
        }
        out.push(cur);
        if split && i < s.len() && s[i] == ',' {
            i += 1;
            if i == s.len() {
                return Err((ErrorCode::EmptyValue, "empty value after a comma"));
            }
            continue;
        }
        return Ok(out);
    }
}

fn normalize(def: &FieldDef, v: &str) -> Result<String, String> {
    match def.kind {
        FieldKind::Severity | FieldKind::Enum => {
            let l = v.to_ascii_lowercase();
            if def.values.contains(&l.as_str()) {
                Ok(l)
            } else {
                Err(format!(
                    "`{v}` is not a {} (one of {})",
                    def.key,
                    def.values.join(", ")
                ))
            }
        }
        FieldKind::Tag => Tag::parse(v).map(String::from).map_err(|e| e.to_string()),
        FieldKind::Cwe => parse_cwe(v)
            .map(|n| format!("CWE-{n}"))
            .ok_or_else(|| format!("`{v}` is not a CWE id (expected 79 or CWE-79)")),
        FieldKind::Text => Ok(v.to_string()),
    }
}

fn parse_token(t: &RawToken, idx: usize) -> Result<Term, ParseError> {
    let e = |code: ErrorCode, message: String| err(idx, t.start, t.end, code, message);
    let (negated, body) = if t.text.len() > 1 && t.text[0] == '-' {
        (true, &t.text[1..])
    } else {
        (false, &t.text[..])
    };
    let key_len = body
        .iter()
        .take_while(|c| c.is_ascii_alphabetic() || **c == '_')
        .count();
    let rest = &body[key_len..];
    let op = if key_len == 0 {
        None
    } else {
        match (rest.first(), rest.get(1)) {
            (Some('>'), Some('=')) => Some((Op::Ge, 2)),
            (Some('<'), Some('=')) => Some((Op::Le, 2)),
            (Some('>'), _) => Some((Op::Gt, 1)),
            (Some('<'), _) => Some((Op::Lt, 1)),
            (Some(':'), _) => Some((Op::Eq, 1)),
            _ => None,
        }
    };
    let Some((op, op_len)) = op else {
        let values = items(body, false).map_err(|(c, m)| e(c, m.to_string()))?;
        return Ok(Term {
            negated,
            key: Key::Text,
            op: Op::Eq,
            values,
        });
    };
    let name: String = body[..key_len].iter().collect();
    let Some(def) = field(&name) else {
        return Err(e(
            ErrorCode::UnknownKey,
            format!("unknown key `{name}` (quote the text to search for it)"),
        ));
    };
    if op != Op::Eq && def.kind != FieldKind::Severity {
        return Err(e(
            ErrorCode::BadOperator,
            format!("`{}` only takes `:`", def.key),
        ));
    }
    let raw = items(&rest[op_len..], true).map_err(|(c, m)| e(c, m.to_string()))?;
    if op != Op::Eq && raw.len() > 1 {
        return Err(e(
            ErrorCode::BadOperator,
            "a comparison takes one value".to_string(),
        ));
    }
    let values = raw
        .iter()
        .map(|v| normalize(def, v))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|m| e(ErrorCode::BadValue, m))?;
    Ok(Term {
        negated,
        key: def.id,
        op,
        values,
    })
}

/// Parse a query. An empty or all-whitespace query has no terms and matches
/// everything.
pub fn parse_query(q: &str) -> Result<ParsedQuery, ParseError> {
    let tokens = scan(q)?;
    let terms = tokens
        .iter()
        .enumerate()
        .map(|(i, t)| parse_token(t, i))
        .collect::<Result<_, _>>()?;
    Ok(ParsedQuery { terms })
}
