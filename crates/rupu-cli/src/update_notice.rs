//! Passive "update available" notice printed (to stderr) on ordinary,
//! interactive invocations. Never blocks and never fails the caller:
//! the notice itself is served from a local cache file
//! (`rupu_update::notice::state_path()`), and a stale cache triggers a
//! best-effort detached background refresh rather than a synchronous
//! network call.

use std::io::IsTerminal as _;

/// Gate for the passive notice: on by default, suppressed for non-TTY,
/// structured output, config `check=false`, or `RUPU_NO_UPDATE_CHECK`.
pub fn should_check(
    cfg_check: Option<bool>,
    env_disabled: bool,
    is_tty: bool,
    structured_output: bool,
) -> bool {
    if env_disabled || structured_output || !is_tty {
        return false;
    }
    cfg_check.unwrap_or(true)
}

/// Print the cached notice (if any, and if newer than `current`), then
/// kick off a detached background refresh when the cache is missing,
/// stale, or was written for a different channel.
///
/// `is_tty` / `structured_output` gate whether anything happens at
/// all; `cfg_check` is `[update].check` from the layered config.
pub fn maybe_print(
    cfg_check: Option<bool>,
    channel: &str,
    current: &str,
    is_tty: bool,
    structured_output: bool,
) {
    let env_disabled = std::env::var_os("RUPU_NO_UPDATE_CHECK").is_some();
    if !should_check(cfg_check, env_disabled, is_tty, structured_output) {
        return;
    }
    let path = rupu_update::notice::state_path();

    // Print from cache first (cheap, no network).
    if let Some(state) = rupu_update::notice::load_state(&path) {
        if state.channel == channel {
            if let Some(line) =
                rupu_update::notice::notice_line(current, &state.latest_version, channel)
            {
                eprintln!("{line}");
            }
        }
    }

    // Refresh in the background when stale — detached, swallow all
    // errors. NOTE (known limitation): this `tokio::spawn` is not
    // awaited or joined anywhere, so on a short-lived command the
    // process can exit before the task completes; the cache only
    // updates opportunistically on runs that stay alive long enough
    // for the GitHub round-trip. Acceptable for a best-effort notice.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let stale = rupu_update::notice::load_state(&path)
        .map(|s| s.channel != channel || rupu_update::notice::is_stale(s.last_checked, now, 86_400))
        .unwrap_or(true);
    if stale {
        let channel = channel.to_string();
        tokio::spawn(async move {
            use rupu_update::model::ReleaseSource as _;
            use std::str::FromStr as _;
            if let Ok(ch) = rupu_update::model::Channel::from_str(&channel) {
                let src = rupu_update::github::GithubReleaseSource::new("Section9Labs/rupu");
                if let Ok(rels) = src.list_releases().await {
                    let plat = rupu_update::decide::current_platform();
                    if let Some(latest) = rupu_update::select::select_latest(&rels, ch, &plat) {
                        let _ = rupu_update::notice::save_state(
                            &path,
                            &rupu_update::notice::CheckState {
                                channel,
                                last_checked: now,
                                latest_version: latest.version.to_string(),
                            },
                        );
                    }
                }
            }
        });
    }
}

/// TTY detection for the notice gate — stderr, matching where the
/// notice itself is printed.
pub fn stderr_is_tty() -> bool {
    std::io::stderr().is_terminal()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_on_for_interactive_tty() {
        assert!(should_check(None, false, true, false));
    }
    #[test]
    fn off_when_not_tty() {
        assert!(!should_check(None, false, false, false));
    }
    #[test]
    fn off_when_structured() {
        assert!(!should_check(Some(true), false, true, true));
    }
    #[test]
    fn off_when_env_disabled() {
        assert!(!should_check(Some(true), true, true, false));
    }
    #[test]
    fn off_when_config_false() {
        assert!(!should_check(Some(false), false, true, false));
    }
}
