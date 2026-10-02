//! The typed locator primitives the core understands.
//!
//! Adding a `Coordinate` variant is the ONLY per-kind change the core ever
//! needs — rare, and shared by every profile. The whole anticipated set is
//! declared up front; an unused variant costs nothing, and pre-declaring keeps
//! every engagement profile pure data from the day it is authored.

use serde::{Deserialize, Serialize};

/// Transport protocol for a [`Coordinate::Port`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Proto {
    Tcp,
    Udp,
    Other,
}

/// One typed locator coordinate. Serialized adjacently tagged (`{"t":..,"v":..}`)
/// so a heterogeneous `Locator` list round-trips unambiguously.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "t", content = "v")]
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
    /// The stable string tag a profile names this coordinate by (in a kind's
    /// `coordinates = [..]` and in a `locator_has_coordinate` predicate).
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

    /// Whether `tag` names a known coordinate. Profiles and predicates are
    /// validated against this so a typo is an error, never a silent miss.
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
}

/// A finding's (or asset's) location: an unordered bag of coordinates. An empty
/// `Locator` is valid — a coordinate-less finding (e.g. a threat-model entry).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Locator(pub Vec<Coordinate>);

impl Locator {
    /// Whether this locator carries a coordinate with the given tag.
    pub fn has(&self, tag: &str) -> bool {
        self.0.iter().any(|c| c.tag() == tag)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tag_matches_known_tag_for_every_variant() {
        let all = [
            Coordinate::Path("p".into()),
            Coordinate::LineRange { start: 1, end: 2 },
            Coordinate::Symbol("s".into()),
            Coordinate::Commit("c".into()),
            Coordinate::Sha256("h".into()),
            Coordinate::Offset(0),
            Coordinate::Address(0),
            Coordinate::Host("h".into()),
            Coordinate::Port {
                number: 443,
                proto: Proto::Tcp,
            },
            Coordinate::Url("u".into()),
            Coordinate::HttpRoute {
                method: "GET".into(),
                path: "/".into(),
            },
            Coordinate::Param("q".into()),
            Coordinate::ResourceId {
                scheme: "arn".into(),
                id: "x".into(),
            },
        ];
        for c in &all {
            assert!(
                Coordinate::known_tag(c.tag()),
                "tag {:?} must be a known tag",
                c.tag()
            );
        }
        // all 13 variants present, tags distinct
        let tags: std::collections::BTreeSet<_> = all.iter().map(|c| c.tag()).collect();
        assert_eq!(tags.len(), 13);
    }

    #[test]
    fn unknown_tag_is_rejected() {
        assert!(!Coordinate::known_tag("cidr"));
        assert!(!Coordinate::known_tag(""));
        assert!(!Coordinate::known_tag("scope"));
    }

    #[test]
    fn round_trips_a_heterogeneous_locator() {
        let loc = Locator(vec![
            Coordinate::Host("10.0.0.1".into()),
            Coordinate::Port {
                number: 22,
                proto: Proto::Tcp,
            },
            Coordinate::LineRange { start: 10, end: 20 },
        ]);
        let json = serde_json::to_string(&loc).unwrap();
        let back: Locator = serde_json::from_str(&json).unwrap();
        assert_eq!(loc, back);
        assert!(loc.has("host") && loc.has("port") && loc.has("line_range"));
        assert!(!loc.has("url"));
    }

    #[test]
    fn empty_locator_is_valid() {
        let loc = Locator::default();
        assert!(loc.0.is_empty());
        assert!(!loc.has("path"));
        let json = serde_json::to_string(&loc).unwrap();
        assert_eq!(serde_json::from_str::<Locator>(&json).unwrap(), loc);
    }
}
