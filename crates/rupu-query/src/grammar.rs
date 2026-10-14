//! The one-line query grammar, generic over a field registry. One line, e.g.
//! `severity>=high tag:class:sqli -tag:false-positive "sql injection"`.
//!
//! Quoting: a quote (`"` or `'`) opens only at an item start (the token body
//! start after an optional `-`, right after a keyed token's operator, or right
//! after a `,` in a keyed value); `\` escapes the next char. See `scan`.
//!
//! This module only parses and validates against a registry of [`FieldDef`]s.
//! The web's `src/lib/findingQuery/grammar.ts` is its twin, held in lockstep
//! by `rupu-coverage`'s `tests/fixtures/finding_query/`. Change both, and the
//! fixtures, together.

use serde::{Serialize, Serializer};

/// A registry-supplied value normalizer: the canonical value, or why `v` is
/// not one.
pub type Normalize = fn(&str) -> Result<String, String>;

/// How a field's values are validated and normalized. Serializes to its
/// `name()`, the `kind` of the `fields.json` lockstep shape.
#[derive(Debug, Clone, Copy)]
pub enum FieldKind {
    /// An ordered ladder (`values`, lowest first): the one kind that takes
    /// the comparison operators `>` `>=` `<` `<=`. Values are lowercased.
    Severity,
    /// One of `values`, lowercased.
    Enum,
    /// Any text, as written.
    Text,
    /// A time: a span back from now (`30m`, `12h`, `7d`, `2w`), a date
    /// (`2026-10-01`) or an RFC 3339 time. See [`crate::time`].
    Time,
    /// A kind the registry's owner defines, named `name` on the wire.
    Custom {
        name: &'static str,
        normalize: Normalize,
    },
}

impl FieldKind {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Severity => "severity",
            Self::Enum => "enum",
            Self::Text => "text",
            Self::Time => "time",
            Self::Custom { name, .. } => name,
        }
    }

    /// Whether a term on this kind may compare (`>=` …) instead of `:`.
    pub fn is_ordered(&self) -> bool {
        matches!(self, Self::Severity)
    }
}

impl Serialize for FieldKind {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.name())
    }
}

/// A queryable field. Serializes to the `fields.json` lockstep shape.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct FieldDef {
    pub key: &'static str,
    pub aliases: &'static [&'static str],
    pub kind: FieldKind,
    pub values: &'static [&'static str],
}

impl FieldDef {
    pub const fn new(key: &'static str, kind: FieldKind) -> Self {
        Self {
            key,
            aliases: &[],
            kind,
            values: &[],
        }
    }

    pub const fn one_of(key: &'static str, values: &'static [&'static str]) -> Self {
        Self {
            key,
            aliases: &[],
            kind: FieldKind::Enum,
            values,
        }
    }

    pub const fn aliased(mut self, aliases: &'static [&'static str]) -> Self {
        self.aliases = aliases;
        self
    }
}

/// The key of a free-text term.
pub const TEXT_KEY: &str = "text";

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
/// value matches, inverted when `negated`. `key` is the field's registry key
/// (an alias resolved), or [`TEXT_KEY`] for free text. Values are normalized
/// per the field's kind.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Term {
    #[serde(rename = "neg")]
    pub negated: bool,
    pub key: &'static str,
    pub op: Op,
    pub values: Vec<String>,
}

/// Every term must match (AND).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ParsedQuery {
    pub terms: Vec<Term>,
}

impl ParsedQuery {
    /// The terms on field `key`.
    pub fn terms_on<'a>(&'a self, key: &'a str) -> impl Iterator<Item = &'a Term> + 'a {
        self.terms.iter().filter(move |t| t.key == key)
    }
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

/// The field named `name` (or one of its aliases) in `fields`,
/// case-insensitively.
pub fn field<'a>(fields: &'a [FieldDef], name: &str) -> Option<&'a FieldDef> {
    let n = name.to_ascii_lowercase();
    fields
        .iter()
        .find(|f| f.key == n || f.aliases.contains(&n.as_str()))
}

struct RawToken {
    start: usize,
    end: usize,
    text: Vec<char>,
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

/// Where a keyed token's value begins: the index right after the operator
/// when the token (after an optional leading `-`) is `[A-Za-z_]+` followed
/// immediately by an operator (`>=` `<=` `:` `>` `<`, longest match first).
/// `parse_token` applies the same detection to the decoded token, so the two
/// always agree on whether a token is keyed.
fn keyed_value_start(chars: &[char], start: usize) -> Option<usize> {
    let body = start + usize::from(chars.get(start) == Some(&'-'));
    let key_len = chars[body..]
        .iter()
        .take_while(|c| c.is_ascii_alphabetic() || **c == '_')
        .count();
    if key_len == 0 {
        return None;
    }
    let k = body + key_len;
    match (chars.get(k), chars.get(k + 1)) {
        (Some('>' | '<'), Some('=')) => Some(k + 2),
        (Some('>' | '<' | ':'), _) => Some(k + 1),
        _ => None,
    }
}

/// Split into tokens at unquoted whitespace. A quote opens only at an item
/// start, and an item starts in exactly three places, the same ones `items`
/// decodes:
///
/// 1. the token body start, after an optional leading `-`;
/// 2. right after the key's operator, when the token is keyed (see
///    `keyed_value_start`);
/// 3. right after an unescaped, unquoted `,` inside a keyed value.
///
/// Nowhere else: not after an operator character in free text (`a="c d"` is
/// the two words `a="c` and `d"`), and not after a second `:` inside a value
/// (`owner:x:"a b"`). `\` escapes the next char, in or out of quotes.
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
        let value_start = keyed_value_start(&chars, start);
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
            item_start = (c == '-' && i == start)
                || value_start.is_some_and(|v| i + 1 == v || (c == ',' && i >= v));
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
                    "`{v}` is not a value of `{}:` (one of {})",
                    def.key,
                    def.values.join(", ")
                ))
            }
        }
        FieldKind::Time => crate::time::normalize(v),
        FieldKind::Custom { normalize, .. } => normalize(v),
        FieldKind::Text => Ok(v.to_string()),
    }
}

fn parse_token(fields: &[FieldDef], t: &RawToken, idx: usize) -> Result<Term, ParseError> {
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
            key: TEXT_KEY,
            op: Op::Eq,
            values,
        });
    };
    let name: String = body[..key_len].iter().collect();
    let Some(def) = field(fields, &name) else {
        return Err(e(
            ErrorCode::UnknownKey,
            format!("unknown key `{name}` (quote the text to search for it)"),
        ));
    };
    if op != Op::Eq && !def.kind.is_ordered() {
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
        key: def.key,
        op,
        values,
    })
}

/// Parse a query against the registry `fields`. An empty or all-whitespace
/// query has no terms and matches everything.
pub fn parse_query(q: &str, fields: &[FieldDef]) -> Result<ParsedQuery, ParseError> {
    let tokens = scan(q)?;
    let terms = tokens
        .iter()
        .enumerate()
        .map(|(i, t)| parse_token(fields, t, i))
        .collect::<Result<_, _>>()?;
    Ok(ParsedQuery { terms })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn upper(v: &str) -> Result<String, String> {
        if v.chars().all(|c| c.is_ascii_alphabetic()) {
            Ok(v.to_ascii_uppercase())
        } else {
            Err(format!("`{v}` has a non-letter"))
        }
    }

    const FIELDS: &[FieldDef] = &[
        FieldDef {
            key: "level",
            aliases: &["lvl"],
            kind: FieldKind::Severity,
            values: &["low", "high"],
        },
        FieldDef::one_of("status", &["open", "closed"]),
        FieldDef::new("name", FieldKind::Text),
        FieldDef::new("since", FieldKind::Time),
        FieldDef::new(
            "code",
            FieldKind::Custom {
                name: "code",
                normalize: upper,
            },
        ),
    ];

    fn terms(q: &str) -> serde_json::Value {
        serde_json::to_value(parse_query(q, FIELDS).unwrap()).unwrap()["terms"].clone()
    }

    #[test]
    fn keys_resolve_aliases_and_values_normalize_per_kind() {
        assert_eq!(
            terms("lvl>=HIGH status:Open,closed name:Ab code:xy since:7D"),
            serde_json::json!([
                {"neg": false, "key": "level", "op": "ge", "values": ["high"]},
                {"neg": false, "key": "status", "op": "eq", "values": ["open", "closed"]},
                {"neg": false, "key": "name", "op": "eq", "values": ["Ab"]},
                {"neg": false, "key": "code", "op": "eq", "values": ["XY"]},
                {"neg": false, "key": "since", "op": "eq", "values": ["7d"]},
            ])
        );
    }

    #[test]
    fn free_text_is_the_text_key() {
        assert_eq!(
            terms("-\"two words\" x"),
            serde_json::json!([
                {"neg": true, "key": "text", "op": "eq", "values": ["two words"]},
                {"neg": false, "key": "text", "op": "eq", "values": ["x"]},
            ])
        );
    }

    #[test]
    fn bad_values_and_operators_name_their_token() {
        let e = parse_query("name:a code:x1", FIELDS).unwrap_err();
        assert_eq!(
            (e.token, e.start, e.end, e.code, e.message.as_str()),
            (1, 7, 14, ErrorCode::BadValue, "`x1` has a non-letter")
        );
        let e = parse_query("since:yesterday", FIELDS).unwrap_err();
        assert_eq!((e.token, e.code), (0, ErrorCode::BadValue));
        let e = parse_query("status>=open", FIELDS).unwrap_err();
        assert_eq!(
            (e.code, e.message.as_str()),
            (ErrorCode::BadOperator, "`status` only takes `:`")
        );
        let e = parse_query("nope:1", FIELDS).unwrap_err();
        assert_eq!(e.code, ErrorCode::UnknownKey);
    }

    #[test]
    fn kinds_serialize_to_their_wire_names() {
        assert_eq!(
            serde_json::to_value(FIELDS).unwrap(),
            serde_json::json!([
                {"key": "level", "aliases": ["lvl"], "kind": "severity", "values": ["low", "high"]},
                {"key": "status", "aliases": [], "kind": "enum", "values": ["open", "closed"]},
                {"key": "name", "aliases": [], "kind": "text", "values": []},
                {"key": "since", "aliases": [], "kind": "time", "values": []},
                {"key": "code", "aliases": [], "kind": "code", "values": []},
            ])
        );
    }
}
