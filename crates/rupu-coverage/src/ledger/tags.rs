//! Finding tags (spec `docs/superpowers/specs/2026-10-06-rupu-finding-tags-design.md`).
//!
//! A finding's tags are the ones it was declared with (`FindingRecord::tags`)
//! plus the add/remove events in its workspace's `finding_tags.jsonl`,
//! applied in file order.

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// At most this many effective tags on one finding.
pub const MAX_TAGS_PER_FINDING: usize = 32;

/// A normalized tag. Constructing one is the only validation path, so a
/// `Tag` in hand is always valid.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Tag(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid tag `{raw}`: {reason}")]
pub struct TagParseError {
    pub raw: String,
    pub reason: &'static str,
}

impl Tag {
    pub const MAX_LEN: usize = 64;

    /// Trim and lowercase `raw`, then validate it. An invalid tag is an
    /// error naming it; it is never rewritten into a valid one.
    pub fn parse(raw: &str) -> Result<Self, TagParseError> {
        let err = |reason| TagParseError {
            raw: raw.to_string(),
            reason,
        };
        let t = raw.trim().to_lowercase();
        if t.is_empty() {
            return Err(err("empty"));
        }
        let allowed = |c: char| {
            c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | ':' | '/' | '-')
        };
        if !t.chars().all(allowed) {
            return Err(err("only a-z, 0-9 and . _ : / - are allowed"));
        }
        if !t.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit()) {
            return Err(err("must start with a letter or digit"));
        }
        // ASCII only from here on, so bytes == characters.
        if t.len() > Self::MAX_LEN {
            return Err(err("longer than 64 characters"));
        }
        Ok(Tag(t))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for Tag {
    type Error = TagParseError;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        Tag::parse(&s)
    }
}

impl From<Tag> for String {
    fn from(t: Tag) -> String {
        t.0
    }
}

impl std::fmt::Display for Tag {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::str::FromStr for Tag {
    type Err = TagParseError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Tag::parse(s)
    }
}

/// Validate every entry; the result is deduped and sorted. The first
/// invalid entry is the error.
pub fn parse_tags<S: AsRef<str>>(raw: &[S]) -> Result<Vec<Tag>, TagParseError> {
    let set: BTreeSet<Tag> = raw
        .iter()
        .map(|r| Tag::parse(r.as_ref()))
        .collect::<Result<_, _>>()?;
    Ok(set.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tag_is_trimmed_and_lowercased() {
        assert_eq!(Tag::parse("  Needs-POC ").unwrap().as_str(), "needs-poc");
        assert_eq!(Tag::parse("class:SQLi").unwrap().as_str(), "class:sqli");
        assert_eq!(
            Tag::parse("team/payments_v2.1").unwrap().as_str(),
            "team/payments_v2.1"
        );
    }

    #[test]
    fn an_invalid_tag_is_rejected_never_rewritten() {
        for (raw, why) in [
            ("", "empty"),
            ("   ", "empty"),
            ("needs poc", "only a-z"),
            ("na\u{ef}ve", "only a-z"),
            ("-leading", "must start"),
            (":leading", "must start"),
        ] {
            let e = Tag::parse(raw).unwrap_err();
            assert!(e.to_string().contains(why), "{raw:?}: {e}");
        }
        assert!(Tag::parse(&"a".repeat(64)).is_ok());
        assert!(Tag::parse(&"a".repeat(65))
            .unwrap_err()
            .to_string()
            .contains("longer than 64"));
    }

    #[test]
    fn parse_tags_dedupes_and_sorts() {
        let tags = parse_tags(&["b", "A", "a", "b"]).unwrap();
        let got: Vec<&str> = tags.iter().map(Tag::as_str).collect();
        assert_eq!(got, ["a", "b"]);
        assert!(parse_tags(&["ok", "not ok"]).is_err());
    }

    #[test]
    fn a_tag_serializes_as_a_plain_string_and_validates_on_read() {
        let t = Tag::parse("needs-poc").unwrap();
        assert_eq!(serde_json::to_string(&t).unwrap(), "\"needs-poc\"");
        assert!(serde_json::from_str::<Tag>("\"bad tag\"").is_err());
    }
}
