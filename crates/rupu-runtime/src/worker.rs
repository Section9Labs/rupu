use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerKind {
    Cli,
    AutoflowServe,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct WorkerCapabilities {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub backends: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scm_hosts: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub permission_modes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerRecord {
    pub version: u32,
    pub worker_id: String,
    pub kind: WorkerKind,
    pub name: String,
    pub host: String,
    #[serde(default)]
    pub capabilities: WorkerCapabilities,
    pub registered_at: String,
    pub last_seen_at: String,
}

impl WorkerRecord {
    pub const VERSION: u32 = 1;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worker_record_round_trips_json() {
        let worker = WorkerRecord {
            version: WorkerRecord::VERSION,
            worker_id: "worker_local_team-mini_cli".into(),
            kind: WorkerKind::Cli,
            name: "team-mini".into(),
            host: "team-mini.local".into(),
            capabilities: WorkerCapabilities {
                backends: vec!["local_worktree".into()],
                scm_hosts: vec!["github".into()],
                permission_modes: vec!["bypass".into(), "readonly".into()],
            },
            registered_at: "2026-05-09T16:00:00Z".into(),
            last_seen_at: "2026-05-09T16:22:00Z".into(),
        };

        let encoded = serde_json::to_string_pretty(&worker).unwrap();
        let decoded: WorkerRecord = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, worker);
    }
}
