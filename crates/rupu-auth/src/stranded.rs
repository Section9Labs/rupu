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

use crate::account_key::{account_for, legacy_account_for};
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
/// Empty on every other OS, and empty on macOS when nothing is found or
/// the keychain cannot be read. Total by construction: this runs inside
/// `rupu auth login`, where a panic or a hard error would be far worse
/// than a missed notice.
pub fn detect_stranded_keychain_credentials() -> Vec<String> {
    if std::env::consts::OS != "macos" {
        return Vec::new();
    }

    LEGACY_PROVIDERS
        .iter()
        .filter(|provider| {
            // Both shapes the keychain ever held: the current
            // `<provider>/<mode>` accounts and the Slice A bare
            // `<provider>` account.
            legacy_accounts_for(**provider)
                .iter()
                .any(|account| keychain_entry_exists(account))
        })
        .map(|p| p.as_str().to_string())
        .collect()
}

/// Every keychain account string a given provider could be stored under.
fn legacy_accounts_for(provider: ProviderId) -> Vec<String> {
    vec![
        account_for(provider, AuthMode::ApiKey),
        account_for(provider, AuthMode::Sso),
        legacy_account_for(provider),
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
            assert!(detect_stranded_keychain_credentials().is_empty());
        }
    }

    #[test]
    fn detection_never_panics_on_the_host_it_runs_on() {
        // Whatever the host, the probe must be total: a missing binary,
        // a locked keychain, and a denied prompt are all "no findings",
        // not a crash in the middle of `rupu auth login`.
        let _ = detect_stranded_keychain_credentials();
    }

    #[test]
    fn probed_accounts_cover_both_historical_shapes() {
        let accounts = legacy_accounts_for(ProviderId::Anthropic);
        assert!(accounts.contains(&"anthropic/api-key".to_string()));
        assert!(accounts.contains(&"anthropic/sso".to_string()));
        assert!(
            accounts.contains(&"anthropic".to_string()),
            "the Slice A bare-provider account must still be probed, or \
             credentials written before the mode suffix go unreported"
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
