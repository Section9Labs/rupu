use crate::catalog::types::{Severity, TouchStrength};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attribution {
    pub run_id: String,
    pub model: String,
    pub surface: Surface,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Surface {
    Workflow,
    Agent,
    Autoflow,
    Session,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum FileTouchEvent {
    Read {
        path: String,
        line_range: [u32; 2],
        tool: String,
        #[serde(flatten)]
        attribution: Attribution,
        at: DateTime<Utc>,
    },
    Grep {
        path: String,
        pattern: String,
        match_count: u32,
        matched_lines: Vec<u32>,
        tool: String,
        #[serde(flatten)]
        attribution: Attribution,
        at: DateTime<Utc>,
    },
    Glob {
        path: String,
        pattern: String,
        tool: String,
        #[serde(flatten)]
        attribution: Attribution,
        at: DateTime<Utc>,
    },
    Edit {
        path: String,
        line_range: [u32; 2],
        lines_changed: u32,
        tool: String,
        #[serde(flatten)]
        attribution: Attribution,
        at: DateTime<Utc>,
    },
    Cmd {
        path: String,
        command: String,
        tool: String,
        #[serde(flatten)]
        attribution: Attribution,
        at: DateTime<Utc>,
    },
    Unknown {
        tool: String,
        arg_hash: String,
        #[serde(flatten)]
        attribution: Attribution,
        at: DateTime<Utc>,
    },
}

impl FileTouchEvent {
    pub fn strength(&self) -> Option<TouchStrength> {
        match self {
            FileTouchEvent::Edit { .. } => Some(TouchStrength::Edit),
            FileTouchEvent::Read { .. } => Some(TouchStrength::Read),
            FileTouchEvent::Grep { .. } => Some(TouchStrength::Grep),
            FileTouchEvent::Cmd { .. } => Some(TouchStrength::Cmd),
            FileTouchEvent::Glob { .. } => Some(TouchStrength::Glob),
            FileTouchEvent::Unknown { .. } => None,
        }
    }

    pub fn path(&self) -> Option<&str> {
        match self {
            FileTouchEvent::Read { path, .. }
            | FileTouchEvent::Grep { path, .. }
            | FileTouchEvent::Glob { path, .. }
            | FileTouchEvent::Edit { path, .. }
            | FileTouchEvent::Cmd { path, .. } => Some(path),
            FileTouchEvent::Unknown { .. } => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssertionStatus {
    Clean,
    Finding,
    Examined,
    NotApplicable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Evidence {
    pub summary: String,
    #[serde(default)]
    pub line_ranges: Vec<[u32; 2]>,
    #[serde(default)]
    pub finding_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConcernAssertion {
    pub concern_id: String,
    pub file_path: String,
    pub status: AssertionStatus,
    pub evidence: Evidence,
    pub declared_by: Attribution,
    pub declared_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FindingScope {
    Line,
    File,
    Repo,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FindingEvidence {
    #[serde(default)]
    pub code_excerpt: Option<String>,
    pub rationale: String,
    #[serde(default)]
    pub references: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FindingRecord {
    pub id: String,
    #[serde(default)]
    pub file_path: Option<String>,
    #[serde(default)]
    pub line_range: Option<[u32; 2]>,
    pub scope: FindingScope,
    pub summary: String,
    pub severity: Severity,
    #[serde(default)]
    pub concern_id: Option<String>,
    pub evidence: FindingEvidence,
    pub declared_by: Attribution,
    pub declared_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attribution() -> Attribution {
        Attribution {
            run_id: "run_01KS19A4MQXP".to_string(),
            model: "claude-sonnet-4-6".to_string(),
            surface: Surface::Workflow,
        }
    }

    #[test]
    fn file_touch_read_event_round_trips_jsonl() {
        let event = FileTouchEvent::Read {
            path: "src/handlers/users.rs".to_string(),
            line_range: [1, 240],
            tool: "read_file".to_string(),
            attribution: attribution(),
            at: DateTime::parse_from_rfc3339("2026-05-23T14:01:32Z").unwrap().with_timezone(&Utc),
        };
        let json = serde_json::to_string(&event).unwrap();
        let decoded: FileTouchEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(event, decoded);
        assert_eq!(event.strength(), Some(TouchStrength::Read));
        assert_eq!(event.path(), Some("src/handlers/users.rs"));
    }

    #[test]
    fn concern_assertion_round_trips_jsonl() {
        let assertion = ConcernAssertion {
            concern_id: "stride:spoofing".to_string(),
            file_path: "src/auth/login.rs".to_string(),
            status: AssertionStatus::Clean,
            evidence: Evidence {
                summary: "Token check covers all entry points.".to_string(),
                line_ranges: vec![[1, 80]],
                finding_ids: vec![],
            },
            declared_by: attribution(),
            declared_at: Utc::now(),
        };
        let json = serde_json::to_string(&assertion).unwrap();
        let decoded: ConcernAssertion = serde_json::from_str(&json).unwrap();
        assert_eq!(assertion, decoded);
    }

    #[test]
    fn finding_record_round_trips_jsonl_with_null_concern() {
        let record = FindingRecord {
            id: "fnd_01KS19A3".to_string(),
            file_path: Some("src/config.rs".to_string()),
            line_range: Some([20, 28]),
            scope: FindingScope::Line,
            summary: "Hardcoded API key.".to_string(),
            severity: Severity::High,
            concern_id: None, // serendipitous
            evidence: FindingEvidence {
                code_excerpt: Some("const STRIPE_KEY = \"sk_live_...\"".to_string()),
                rationale: "Key should come from env.".to_string(),
                references: vec!["https://cwe.mitre.org/data/definitions/798.html".to_string()],
            },
            declared_by: attribution(),
            declared_at: Utc::now(),
        };
        let json = serde_json::to_string(&record).unwrap();
        let decoded: FindingRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(record, decoded);
    }
}
