//! The typed locator primitives the core understands. Adding a variant is the
//! only per-kind reason to touch core code — rare, shared by every profile.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Proto {
    Tcp,
    Udp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Coordinate {
    // static / code / binary
    Path(String),
    LineRange { start: u32, end: u32 },
    Symbol(String),
    Commit(String),
    Sha256(String),
    Offset(u64),
    Address(u64),
    // network
    Host(String),
    Port { number: u16, proto: Proto },
    Url(String),
    // web / appsec
    HttpRoute { method: String, path: String },
    Param(String),
    // cloud / saas / k8s
    ResourceId { scheme: String, id: String },
}

impl Coordinate {
    pub fn tag(&self) -> &'static str {
        match self {
            Coordinate::Path(_) => "path",
            Coordinate::LineRange { .. } => "line_range",
            Coordinate::Symbol(_) => "symbol",
            Coordinate::Commit(_) => "commit",
            Coordinate::Sha256(_) => "sha256",
            Coordinate::Offset(_) => "offset",
            Coordinate::Address(_) => "address",
            Coordinate::Host(_) => "host",
            Coordinate::Port { .. } => "port",
            Coordinate::Url(_) => "url",
            Coordinate::HttpRoute { .. } => "http_route",
            Coordinate::Param(_) => "param",
            Coordinate::ResourceId { .. } => "resource_id",
        }
    }

    /// Every tag a profile may name in a kind's `coordinates` list.
    pub fn known_tag(tag: &str) -> bool {
        matches!(
            tag,
            "path"
                | "line_range"
                | "symbol"
                | "commit"
                | "sha256"
                | "offset"
                | "address"
                | "host"
                | "port"
                | "url"
                | "http_route"
                | "param"
                | "resource_id"
        )
    }

    /// The coordinate's value as it reads in a label (addresses in hex).
    pub fn display_value(&self) -> String {
        match self {
            Coordinate::Path(v)
            | Coordinate::Symbol(v)
            | Coordinate::Commit(v)
            | Coordinate::Sha256(v)
            | Coordinate::Host(v)
            | Coordinate::Url(v)
            | Coordinate::Param(v) => v.clone(),
            Coordinate::LineRange { start, end } => format!("{start}-{end}"),
            Coordinate::Offset(n) | Coordinate::Address(n) => format!("0x{n:x}"),
            Coordinate::Port { number, proto } => format!(
                "{number}/{}",
                match proto {
                    Proto::Tcp => "tcp",
                    Proto::Udp => "udp",
                }
            ),
            Coordinate::HttpRoute { method, path } => format!("{method} {path}"),
            Coordinate::ResourceId { scheme, id } => format!("{scheme}:{id}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Locator(pub Vec<Coordinate>);

impl Locator {
    pub fn has(&self, tag: &str) -> bool {
        self.0.iter().any(|c| c.tag() == tag)
    }

    /// Fill a profile kind's label template (`"{symbol} @ {address}"`): each
    /// `{tag}` becomes the value of this locator's coordinate with that tag.
    /// `None` when the template names a coordinate the locator lacks, so the
    /// caller can fall back rather than show a half-rendered label.
    pub fn render_label(&self, template: &str) -> Option<String> {
        let mut out = String::new();
        let mut rest = template;
        while let Some(open) = rest.find('{') {
            out.push_str(&rest[..open]);
            let after = &rest[open + 1..];
            let Some(close) = after.find('}') else {
                // An unclosed brace is literal text.
                out.push_str(&rest[open..]);
                return Some(out);
            };
            let tag = &after[..close];
            out.push_str(&self.0.iter().find(|c| c.tag() == tag)?.display_value());
            rest = &after[close + 1..];
        }
        out.push_str(rest);
        Some(out)
    }

    /// Every coordinate's value, space-separated: the label of last resort
    /// when a kind's template cannot be filled.
    pub fn describe(&self) -> String {
        self.0
            .iter()
            .map(Coordinate::display_value)
            .collect::<Vec<_>>()
            .join(" ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tag_and_has_and_roundtrip() {
        let loc = Locator(vec![
            Coordinate::Host("10.0.0.1".into()),
            Coordinate::Port {
                number: 443,
                proto: Proto::Tcp,
            },
        ]);
        assert!(loc.has("host") && loc.has("port") && !loc.has("url"));
        assert_eq!(Coordinate::Address(0x401000).tag(), "address");
        let j = serde_json::to_string(&loc).unwrap();
        assert_eq!(serde_json::from_str::<Locator>(&j).unwrap(), loc);
    }

    #[test]
    fn render_label_fills_tags_and_declines_a_missing_one() {
        let loc = Locator(vec![
            Coordinate::Address(0x401000),
            Coordinate::Symbol("main".into()),
        ]);
        assert_eq!(
            loc.render_label("{symbol} @ {address}").as_deref(),
            Some("main @ 0x401000")
        );
        assert_eq!(loc.render_label("plain").as_deref(), Some("plain"));
        assert_eq!(
            loc.render_label("{symbol} {oops").as_deref(),
            Some("main {oops")
        );
        assert_eq!(loc.render_label("{path}"), None, "no such coordinate");
        assert_eq!(loc.describe(), "0x401000 main");
        assert_eq!(Locator(vec![]).describe(), "");
    }

    /// One value per `Coordinate` variant. If you add a variant, add it here —
    /// the exhaustive match in `tag()` forces the other half of the contract.
    fn one_of_each() -> Vec<Coordinate> {
        vec![
            Coordinate::Path("src/lib.rs".into()),
            Coordinate::LineRange { start: 1, end: 9 },
            Coordinate::Symbol("main".into()),
            Coordinate::Commit("deadbeef".into()),
            Coordinate::Sha256("00".repeat(32)),
            Coordinate::Offset(16),
            Coordinate::Address(0x401000),
            Coordinate::Host("10.0.0.1".into()),
            Coordinate::Port {
                number: 443,
                proto: Proto::Tcp,
            },
            Coordinate::Url("https://example.test/".into()),
            Coordinate::HttpRoute {
                method: "GET".into(),
                path: "/v1/things".into(),
            },
            Coordinate::Param("q".into()),
            Coordinate::ResourceId {
                scheme: "arn".into(),
                id: "aws:s3:::bucket".into(),
            },
        ]
    }

    #[test]
    fn tag_known_tag_and_serde_key_agree_for_every_variant() {
        let all = one_of_each();
        assert_eq!(all.len(), 13, "one value per Coordinate variant");
        for c in &all {
            assert!(
                Coordinate::known_tag(c.tag()),
                "known_tag rejects tag() {:?} of {c:?}",
                c.tag()
            );
            let v = serde_json::to_value(c).unwrap();
            let obj = v.as_object().expect("externally tagged => object");
            assert_eq!(obj.len(), 1, "{c:?} => {v}");
            let key = obj.keys().next().unwrap();
            assert_eq!(key, c.tag(), "serde key drifted from tag() for {c:?}");
        }
    }
}
