use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CheckState {
    pub channel: String,
    pub last_checked: u64,
    pub latest_version: String,
}

pub fn state_path() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    home.join(".rupu").join("update-check.json")
}

pub fn load_state(path: &Path) -> Option<CheckState> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

pub fn save_state(path: &Path, s: &CheckState) -> Result<(), crate::UpdateError> {
    if let Some(p) = path.parent() {
        std::fs::create_dir_all(p)?;
    }
    let text =
        serde_json::to_string_pretty(s).map_err(|e| crate::UpdateError::Parse(e.to_string()))?;
    std::fs::write(path, text)?;
    Ok(())
}

pub fn is_stale(last_checked: u64, now: u64, ttl_secs: u64) -> bool {
    now.saturating_sub(last_checked) > ttl_secs
}

/// One-line notice, only when `latest` > `current` (semver). None otherwise.
pub fn notice_line(current: &str, latest: &str, channel: &str) -> Option<String> {
    let cur = semver::Version::parse(current).ok()?;
    let lat = semver::Version::parse(latest).ok()?;
    if lat > cur {
        Some(format!(
            "A new rupu is available: {current} → {latest} ({channel}). Run 'rupu update'."
        ))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("update-check.json");
        let s = CheckState {
            channel: "beta".into(),
            last_checked: 100,
            latest_version: "0.35.4".into(),
        };
        save_state(&p, &s).unwrap();
        assert_eq!(load_state(&p).unwrap(), s);
    }
    #[test]
    fn staleness() {
        assert!(is_stale(0, 90_000, 86_400));
        assert!(!is_stale(90_000, 100_000, 86_400));
    }
    #[test]
    fn notice_only_when_newer() {
        assert!(notice_line("0.35.3", "0.35.4", "stable")
            .unwrap()
            .contains("→ 0.35.4"));
        assert!(notice_line("0.35.4", "0.35.4", "stable").is_none());
        assert!(notice_line("0.35.5", "0.35.4", "beta").is_none());
    }
}
