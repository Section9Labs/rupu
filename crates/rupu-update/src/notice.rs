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
///
/// `call_to_action` overrides the trailing "Run 'rupu update'." sentence.
/// Callers use this on a packaged install, where `rupu update` itself
/// refuses and the default call-to-action would contradict it — the
/// override then names the package manager instead. `rupu-update` has no
/// way to know it is packaged (that lives in `rupu-cli`'s `build_info`),
/// so the computed replacement text is handed in rather than looked up
/// here, keeping this crate free of a dependency back up to `rupu-cli`.
pub fn notice_line(
    current: &str,
    latest: &str,
    channel: &str,
    call_to_action: Option<&str>,
) -> Option<String> {
    let cur = semver::Version::parse(current).ok()?;
    let lat = semver::Version::parse(latest).ok()?;
    if lat > cur {
        let cta = call_to_action.unwrap_or("Run 'rupu update'.");
        Some(format!(
            "A new rupu is available: {current} → {latest} ({channel}). {cta}"
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
        assert!(notice_line("0.35.3", "0.35.4", "stable", None)
            .unwrap()
            .contains("→ 0.35.4"));
        assert!(notice_line("0.35.4", "0.35.4", "stable", None).is_none());
        assert!(notice_line("0.35.5", "0.35.4", "beta", None).is_none());
    }

    #[test]
    fn default_call_to_action_is_rupu_update() {
        let line = notice_line("0.35.3", "0.35.4", "stable", None).unwrap();
        assert!(line.ends_with("Run 'rupu update'."), "message was: {line}");
    }

    #[test]
    fn call_to_action_override_replaces_the_default() {
        let line = notice_line(
            "0.35.3",
            "0.35.4",
            "stable",
            Some("Run 'sudo apt upgrade rupu'."),
        )
        .unwrap();
        assert!(
            line.ends_with("Run 'sudo apt upgrade rupu'."),
            "message was: {line}"
        );
        assert!(!line.contains("Run 'rupu update'."), "message was: {line}");
    }
}
