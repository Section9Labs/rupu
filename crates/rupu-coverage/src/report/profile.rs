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
}

#[cfg(test)]
mod tests {
    use super::FindingProfile::{self, Full, Summary};

    #[test]
    fn resolve_prefers_the_most_specific_setting() {
        assert_eq!(FindingProfile::resolve(Some(Summary), Some(Full), Some(Full)), Summary);
        assert_eq!(FindingProfile::resolve(None, Some(Summary), Some(Full)), Summary);
        assert_eq!(FindingProfile::resolve(None, None, Some(Summary)), Summary);
        assert_eq!(FindingProfile::resolve(None, None, None), Full);
    }

    #[test]
    fn serializes_lowercase() {
        assert_eq!(serde_json::to_string(&Full).unwrap(), "\"full\"");
        assert_eq!(serde_json::from_str::<FindingProfile>("\"summary\"").unwrap(), Summary);
    }
}
