//! Platform identifiers for SCM and issue-tracker hosts.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    Github,
    Gitlab,
}

impl Platform {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Github => "github",
            Self::Gitlab => "gitlab",
        }
    }
}

impl fmt::Display for Platform {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Platform {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "github" => Ok(Self::Github),
            "gitlab" => Ok(Self::Gitlab),
            other => Err(format!("unknown platform: {other}")),
        }
    }
}

/// Issue trackers. B-2 only ships connectors for `Github` and `Gitlab`;
/// `Linear` and `Jira` exist so future adapters slot in without
/// reshaping call sites. Code that matches on this enum must include
/// `_ => Err(NotWiredInV0(...))` arms for the unbuilt variants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IssueTracker {
    Github,
    Gitlab,
    Linear,
    Jira,
}

impl IssueTracker {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Github => "github",
            Self::Gitlab => "gitlab",
            Self::Linear => "linear",
            Self::Jira => "jira",
        }
    }
}

impl fmt::Display for IssueTracker {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for IssueTracker {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "github" => Ok(Self::Github),
            "gitlab" => Ok(Self::Gitlab),
            "linear" => Ok(Self::Linear),
            "jira" => Ok(Self::Jira),
            other => Err(format!("unknown issue tracker: {other}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_round_trips_strings() {
        for (p, s) in [(Platform::Github, "github"), (Platform::Gitlab, "gitlab")] {
            assert_eq!(p.as_str(), s);
            assert_eq!(p.to_string(), s);
            assert_eq!(Platform::from_str(s).unwrap(), p);
        }
        assert!(Platform::from_str("bogus").is_err());
    }

    #[test]
    fn platform_serde_lowercase() {
        let json = serde_json::to_string(&Platform::Github).unwrap();
        assert_eq!(json, "\"github\"");
        let p: Platform = serde_json::from_str(&json).unwrap();
        assert_eq!(p, Platform::Github);
    }

    #[test]
    fn issue_tracker_includes_all_four_variants() {
        for (t, s) in [
            (IssueTracker::Github, "github"),
            (IssueTracker::Gitlab, "gitlab"),
            (IssueTracker::Linear, "linear"),
            (IssueTracker::Jira, "jira"),
        ] {
            assert_eq!(t.as_str(), s);
            assert_eq!(IssueTracker::from_str(s).unwrap(), t);
        }
    }
}
