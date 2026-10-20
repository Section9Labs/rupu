//! What `rupu auth status` and `rupu scm accounts` report, shared with the
//! CP's `GET /api/auth/status` and `GET /api/scm/accounts`: the accounts a
//! config declares (plus the built-in vendor names), their kinds, which
//! `[[scm.rules]]` select them and whether a credential is stored for each.
//!
//! Nothing here reads or returns any part of a credential: the resolver is
//! only asked whether one exists and, for an SSO token, when it expires.

use chrono::{DateTime, Utc};
use rupu_auth::KeychainResolver;
use rupu_scm::Platform;
use serde::Serialize;

/// The vendor names `auth status` always lists, so a single-account user
/// who declared nothing still sees every vendor.
pub const BUILTIN_ACCOUNTS: &[&str] = &[
    "anthropic",
    "openai",
    "gemini",
    "copilot",
    "github",
    "gitlab",
    "linear",
    "jira",
];

/// One account's credential state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AccountAuth {
    pub account: String,
    /// The vendor it talks to (`anthropic`, `openai-compatible`, `github`,
    /// …); `None` when the config names no kind and the name is no vendor.
    pub kind: Option<String>,
    /// A stored API key, or `RUPU_<ACCOUNT>_API_KEY` set in this process's
    /// environment.
    pub api_key: bool,
    /// The stored SSO token's expiry, when there is one.
    pub sso: Option<SsoStatus>,
    pub state: AuthState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SsoStatus {
    pub expires_at: Option<DateTime<Utc>>,
    pub expired: bool,
    /// What `rupu auth status` prints: `expires in 8d`, `expired — re-login`,
    /// `no expiry`.
    pub summary: String,
}

/// Whether the account can authenticate as it stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthState {
    /// An API key, or an SSO token that has not expired.
    Ok,
    /// Only an expired SSO token: `rupu auth login` again.
    NeedsLogin,
    /// No credential at all.
    Absent,
}

/// The account's kind: its `[providers.<name>]` / `[scm.<name>]` `kind`,
/// else the name itself when it is a vendor. [`provider_factory::resolve_kind`]
/// covers LLM providers only; this adds the `[scm.*]` half the same way
/// `rupu_scm::Registry::discover` resolves it, so the two never drift.
///
/// [`provider_factory::resolve_kind`]: crate::provider_factory::resolve_kind
pub fn declared_kind(cfg: &rupu_config::Config, name: &str) -> Option<String> {
    if let Some(k) = crate::provider_factory::resolve_kind(name, &cfg.providers) {
        return Some(k);
    }
    cfg.scm.platforms.get(name).and_then(|p| {
        p.kind.clone().or_else(|| {
            name.parse::<Platform>()
                .ok()
                .map(|k| k.as_str().to_string())
        })
    })
}

/// [`BUILTIN_ACCOUNTS`], then every declared `[providers.<name>]` and
/// `[scm.<name>]` account in name order, each once.
pub fn account_names(cfg: &rupu_config::Config) -> Vec<String> {
    let mut names: Vec<String> = BUILTIN_ACCOUNTS.iter().map(|s| s.to_string()).collect();
    let mut declared: std::collections::BTreeSet<String> = cfg.providers.keys().cloned().collect();
    declared.extend(cfg.scm.platforms.keys().cloned());
    for name in declared {
        if !names.contains(&name) {
            names.push(name);
        }
    }
    names
}

/// One account's [`AccountAuth`].
pub async fn account_auth(
    cfg: &rupu_config::Config,
    resolver: &KeychainResolver,
    name: &str,
) -> AccountAuth {
    let api_key = resolver
        .peek_named(name, rupu_providers::AuthMode::ApiKey)
        .await
        || KeychainResolver::has_env_api_key(name);
    let sso = resolver
        .peek_sso_status_named(name)
        .await
        .map(|p| SsoStatus {
            expired: p.expires_at.is_some_and(|at| at <= Utc::now()),
            expires_at: p.expires_at,
            summary: p.summary,
        });
    let state = match (&sso, api_key) {
        (_, true) => AuthState::Ok,
        (Some(s), false) if !s.expired => AuthState::Ok,
        (Some(_), false) => AuthState::NeedsLogin,
        (None, false) => AuthState::Absent,
    };
    AccountAuth {
        account: name.to_string(),
        kind: declared_kind(cfg, name),
        api_key,
        sso,
        state,
    }
}

/// `rupu auth status`: every account of [`account_names`].
pub async fn auth_status(
    cfg: &rupu_config::Config,
    resolver: &KeychainResolver,
) -> Vec<AccountAuth> {
    let mut rows = Vec::new();
    for name in account_names(cfg) {
        rows.push(account_auth(cfg, resolver, &name).await);
    }
    rows
}

/// A configured repo-platform account (github/gitlab) — the accounts
/// `[[scm.rules]]` route between.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ScmAccount {
    pub account: String,
    pub kind: Platform,
    pub base_url: Option<String>,
    /// The rules that select it, in config order.
    pub rules: Vec<ScmRuleTarget>,
}

/// What a `[[scm.rules]]` entry matches on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ScmRuleTarget {
    Owner(String),
    Path(String),
}

impl std::fmt::Display for ScmRuleTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ScmRuleTarget::Owner(o) => write!(f, "owner={o}"),
            ScmRuleTarget::Path(p) => write!(f, "path={p}"),
        }
    }
}

/// `rupu scm accounts`: `github` and `gitlab` first, then every declared
/// `[scm.<name>]` table whose kind (its `kind`, else its name) is a repo
/// platform. Linear/Jira tables and typos are not repo accounts and are
/// skipped, as `rupu_scm::Registry::discover` skips them.
///
/// Config-derived on purpose: it answers "what does my config declare",
/// with no credential and no network call. A listed account may have no
/// working credential.
pub fn scm_accounts(cfg: &rupu_config::Config) -> Vec<ScmAccount> {
    let mut names: Vec<String> = vec!["github".to_string(), "gitlab".to_string()];
    for name in cfg.scm.platforms.keys() {
        if !names.contains(name) {
            names.push(name.clone());
        }
    }
    let mut rows = Vec::new();
    for name in names {
        let platform_cfg = cfg.scm.platforms.get(&name);
        let kind = platform_cfg
            .and_then(|p| p.kind.as_deref())
            .and_then(|k| k.parse::<Platform>().ok())
            .or_else(|| name.parse::<Platform>().ok());
        let Some(kind) = kind else { continue };
        let rules = cfg
            .scm
            .rules
            .iter()
            .filter(|rule| rule.account == name)
            .filter_map(|rule| match (&rule.owner, &rule.path) {
                (Some(o), _) => Some(ScmRuleTarget::Owner(o.clone())),
                (None, Some(p)) => Some(ScmRuleTarget::Path(p.clone())),
                (None, None) => None,
            })
            .collect();
        rows.push(ScmAccount {
            base_url: platform_cfg.and_then(|p| p.base_url.clone()),
            account: name,
            kind,
            rules,
        });
    }
    rows
}

/// Whether `account` is a declared `[scm.*]` table or a bare vendor name:
/// what `rupu scm bind` checks before warning that a rule may never match.
pub fn is_known_scm_account(cfg: &rupu_config::Config, account: &str) -> bool {
    cfg.scm.platforms.contains_key(account) || account.parse::<Platform>().is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn declared_kind_resolves_an_scm_declared_account() {
        let cfg: rupu_config::Config =
            toml::from_str("[scm.\"gh.work\"]\nkind = \"github\"\n[scm.gitlab]\n").unwrap();
        assert_eq!(declared_kind(&cfg, "gh.work").as_deref(), Some("github"));
        assert_eq!(declared_kind(&cfg, "gitlab").as_deref(), Some("gitlab"));
        assert_eq!(
            declared_kind(&cfg, "anthropic").as_deref(),
            Some("anthropic")
        );
        assert_eq!(declared_kind(&cfg, "nope"), None);
    }

    #[test]
    fn scm_accounts_lists_vendors_then_declared_repo_accounts_with_their_rules() {
        let cfg: rupu_config::Config = toml::from_str(
            "[scm.\"gh.work\"]\nkind = \"github\"\nbase_url = \"https://ghe.example\"\n\
             [scm.linear]\n\
             [[scm.rules]]\nowner = \"acme/*\"\naccount = \"gh.work\"\n\
             [[scm.rules]]\npath = \"~/w/*\"\naccount = \"gh.work\"\n",
        )
        .unwrap();
        let rows = scm_accounts(&cfg);
        let names: Vec<&str> = rows.iter().map(|r| r.account.as_str()).collect();
        assert_eq!(names, ["github", "gitlab", "gh.work"]);
        let work = &rows[2];
        assert_eq!(work.kind, Platform::Github);
        assert_eq!(work.base_url.as_deref(), Some("https://ghe.example"));
        assert_eq!(
            work.rules,
            [
                ScmRuleTarget::Owner("acme/*".into()),
                ScmRuleTarget::Path("~/w/*".into())
            ]
        );
        assert!(is_known_scm_account(&cfg, "gh.work"));
        assert!(is_known_scm_account(&cfg, "gitlab"));
        assert!(!is_known_scm_account(&cfg, "gh"));
    }
}
