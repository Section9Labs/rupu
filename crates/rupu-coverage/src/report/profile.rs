//! Which findings contract a run records under.

use serde::{Deserialize, Serialize};

/// `full` requires a complete [`crate::report::FindingReport`]; `summary` is
/// the lightweight record (summary + severity + evidence) rupu has always
/// written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FindingProfile {
    /// The built-in default: a finding is a full report.
    #[default]
    Full,
    Summary,
}

impl FindingProfile {
    /// The profile a ledger line gets when it predates profiles. Every such
    /// line was written as a summary record.
    pub fn legacy() -> Self {
        Self::Summary
    }

    /// Precedence, most specific first: step → workflow `defaults` → agent
    /// frontmatter → built-in default ([`FindingProfile::Full`]).
    pub fn resolve(step: Option<Self>, workflow: Option<Self>, agent: Option<Self>) -> Self {
        step.or(workflow).or(agent).unwrap_or_default()
    }

    /// The wire name: exactly what serde, workflow YAML, agent frontmatter,
    /// and `rupu run --findings-profile` use.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Summary => "summary",
        }
    }
}

impl std::fmt::Display for FindingProfile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Exact, case-sensitive — the same names serde accepts.
impl std::str::FromStr for FindingProfile {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "full" => Ok(Self::Full),
            "summary" => Ok(Self::Summary),
            other => Err(format!(
                "unknown findings profile {other:?}: expected `full` or `summary`"
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::FindingProfile::{self, Full, Summary};

    #[test]
    fn resolve_prefers_the_most_specific_setting() {
        assert_eq!(
            FindingProfile::resolve(Some(Summary), Some(Full), Some(Full)),
            Summary
        );
        assert_eq!(
            FindingProfile::resolve(None, Some(Summary), Some(Full)),
            Summary
        );
        assert_eq!(FindingProfile::resolve(None, None, Some(Summary)), Summary);
        assert_eq!(FindingProfile::resolve(None, None, None), Full);
    }

    #[test]
    fn parses_and_prints_the_wire_names() {
        assert_eq!("full".parse::<FindingProfile>().unwrap(), Full);
        assert_eq!("summary".parse::<FindingProfile>().unwrap(), Summary);
        assert_eq!(Full.as_str(), "full");
        assert_eq!(Summary.to_string(), "summary");
        let err = "Summary".parse::<FindingProfile>().unwrap_err();
        assert!(err.contains("`full` or `summary`"), "{err}");
        assert!("".parse::<FindingProfile>().is_err());
    }

    #[test]
    fn serializes_lowercase() {
        assert_eq!(serde_json::to_string(&Full).unwrap(), "\"full\"");
        assert_eq!(
            serde_json::from_str::<FindingProfile>("\"summary\"").unwrap(),
            Summary
        );
    }
}
