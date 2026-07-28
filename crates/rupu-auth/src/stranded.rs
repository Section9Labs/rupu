//! Detects credentials left behind in the macOS keychain by rupu
//! versions that stored them there.
//!
//! Deliberately a `security` shellout rather than a `keyring`
//! dependency: keeping the keyring crate alive purely for migration
//! would preserve exactly the complexity its removal exists to shed.
//!
//! Read-only. Nothing is imported and nothing is deleted — the user
//! re-runs `rupu auth login`, which writes to the file store. An
//! automatic import would need the whole keychain addressing scheme
//! kept alive, and has no clean equivalent off macOS, so it would
//! silently do nothing for some users while appearing to work.

use crate::backend::ProviderId;
use rupu_providers::AuthMode;

/// Keychain service rupu wrote its entries under.
const LEGACY_SERVICE: &str = "rupu";

/// Providers that could have credentials in the old keychain store.
/// Mirrors `ProviderId`'s variants as of the retirement; a provider
/// added later cannot have a legacy entry by definition.
const LEGACY_PROVIDERS: &[ProviderId] = &[
    ProviderId::Anthropic,
    ProviderId::Openai,
    ProviderId::Gemini,
    ProviderId::Copilot,
    ProviderId::Github,
    ProviderId::Gitlab,
    ProviderId::Linear,
    ProviderId::Jira,
    ProviderId::Local,
];

/// Provider names with credentials still sitting in the macOS keychain.
///
/// `named` supplies user-defined (openai-compatible) provider names from
/// config — `store_named` wrote those under the same `<name>/<mode>`
/// accounts, so a detector that only knew the `ProviderId` built-ins
/// would silently miss them. Callers that have no config loaded may pass
/// an empty slice.
///
/// Empty on every other OS, and empty on macOS when nothing is found or
/// the keychain cannot be read. Total by construction: this runs inside
/// `rupu auth login`, where a panic or a hard error would be far worse
/// than a missed notice.
pub fn detect_stranded_keychain_credentials(named: &[String]) -> Vec<String> {
    if std::env::consts::OS != "macos" {
        return Vec::new();
    }

    let builtins = LEGACY_PROVIDERS
        .iter()
        .map(|p| p.as_str().to_string())
        .collect::<Vec<_>>();

    let mut seen = std::collections::BTreeSet::new();
    builtins
        .iter()
        .chain(named.iter())
        // A named provider may shadow a built-in name; probing it twice
        // would report it twice in the notice.
        .filter(|name| seen.insert((*name).clone()))
        .filter(|name| {
            accounts_for_name(name)
                .iter()
                .any(|account| keychain_entry_exists(account))
        })
        .cloned()
        .collect()
}

/// Every keychain account string a provider could be stored under: the
/// current `<name>/<mode>` shapes plus the Slice A bare `<name>`.
///
/// Built on the plain provider string rather than `ProviderId` so named
/// providers, which have no enum variant, go through the identical path.
fn accounts_for_name(name: &str) -> Vec<String> {
    vec![
        format!("{name}/{}", AuthMode::ApiKey.as_str()),
        format!("{name}/{}", AuthMode::Sso.as_str()),
        name.to_string(),
    ]
}

/// `security find-generic-password -s rupu -a <account>` exits 0 when an
/// entry exists. Output is discarded — the exit status is the whole
/// signal, and `-w` (which would print the secret) is deliberately not
/// passed, so this never reads a password and never prompts for access.
fn keychain_entry_exists(account: &str) -> bool {
    std::process::Command::new("security")
        .args(["find-generic-password", "-s", LEGACY_SERVICE, "-a", account])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detection_is_empty_off_macos() {
        // The probe shells out to `security`, which exists only on macOS.
        // Everywhere else this must be a no-op returning no findings —
        // never an error, never a spurious warning.
        if std::env::consts::OS != "macos" {
            assert!(detect_stranded_keychain_credentials(&[]).is_empty());
        }
    }

    #[test]
    fn detection_never_panics_on_the_host_it_runs_on() {
        // Whatever the host, the probe must be total: a missing binary,
        // a locked keychain, and a denied prompt are all "no findings",
        // not a crash in the middle of `rupu auth login`.
        let _ = detect_stranded_keychain_credentials(&[]);
        let _ = detect_stranded_keychain_credentials(&["oracle".to_string()]);
    }

    #[test]
    fn probed_accounts_cover_both_historical_shapes() {
        let accounts = accounts_for_name(ProviderId::Anthropic.as_str());
        assert!(accounts.contains(&"anthropic/api-key".to_string()));
        assert!(accounts.contains(&"anthropic/sso".to_string()));
        assert!(
            accounts.contains(&"anthropic".to_string()),
            "the Slice A bare-provider account must still be probed, or \
             credentials written before the mode suffix go unreported"
        );
    }

    /// Named (openai-compatible) providers have no `ProviderId` variant,
    /// but `store_named` wrote them under the same account shapes. They
    /// must be probed identically or their credentials strand silently.
    #[test]
    fn named_providers_get_the_same_account_shapes_as_builtins() {
        let named = accounts_for_name("oracle");
        assert!(named.contains(&"oracle/api-key".to_string()));
        assert!(named.contains(&"oracle/sso".to_string()));
        assert!(named.contains(&"oracle".to_string()));

        // Identical shape to a built-in, modulo the name.
        let builtin = accounts_for_name(ProviderId::Anthropic.as_str());
        assert_eq!(named.len(), builtin.len());
    }

    /// A named provider configured with the same name as a built-in
    /// would otherwise be probed twice and reported twice. Hermetic:
    /// trivially true when nothing is stranded, catches the duplicate
    /// when something is.
    #[test]
    fn results_never_contain_duplicates() {
        let out =
            detect_stranded_keychain_credentials(&["anthropic".to_string(), "oracle".to_string()]);
        let mut deduped = out.clone();
        deduped.sort();
        deduped.dedup();
        assert_eq!(
            out.len(),
            deduped.len(),
            "a named provider shadowing a built-in must not be reported twice: {out:?}"
        );
    }

    #[test]
    fn a_nonexistent_account_is_not_reported_as_present() {
        // Guards the exit-status interpretation: if this ever returns
        // true, every provider would be reported as stranded and the
        // notice would fire for everyone.
        assert!(!keychain_entry_exists(
            "rupu-definitely-not-a-real-account-zzz"
        ));
    }
}
