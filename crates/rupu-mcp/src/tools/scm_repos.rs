//! scm.repos.{list, get} tools.

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;

use super::{ToolKind, ToolSpec};
use crate::error::McpError;
use rupu_scm::{AccountId, Platform, Registry, RepoRef};

#[derive(Deserialize, JsonSchema)]
pub struct ListReposArgs {
    /// Platform to query (`github` | `gitlab`). Omit to use [scm.default].
    pub platform: Option<String>,
    /// Which configured account to fan out over, when more than one is
    /// configured for this platform (e.g. two GitHub accounts). Omit to
    /// list every configured account of `platform` at once — each repo
    /// in the response is tagged with the `account` that returned it.
    pub account: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct GetRepoArgs {
    pub platform: Option<String>,
    pub owner: String,
    pub repo: String,
    /// Which configured account to use, when more than one is configured
    /// for this platform (e.g. two GitHub accounts). Only needed when
    /// `[[scm.rules]]` don't disambiguate.
    pub account: Option<String>,
}

pub fn specs() -> Vec<ToolSpec> {
    vec![
        ToolSpec {
            name: "scm.repos.list",
            description: "List repositories the authenticated user can access on the given platform. Omit `platform` to use [scm.default].",
            input_schema: serde_json::to_value(schemars::schema_for!(ListReposArgs)).unwrap(),
            kind: ToolKind::Read,
        },
        ToolSpec {
            name: "scm.repos.get",
            description: "Fetch a single repository (default branch, clone URLs, visibility, description).",
            input_schema: serde_json::to_value(schemars::schema_for!(GetRepoArgs)).unwrap(),
            kind: ToolKind::Read,
        },
    ]
}

/// A row of `scm.repos.list`'s response: the repo plus which configured
/// account it came from. `account` is additive relative to the bare
/// `Repo` shape returned before Arc 2 — every existing field of `Repo`
/// is still present, flattened, so an agent that only reads
/// `default_branch`/`clone_url_https`/etc. is unaffected; only an agent
/// that needs to disambiguate two accounts of the same platform (e.g.
/// to pass `account` back into `scm.repos.get`) needs to read the new
/// field.
#[derive(serde::Serialize)]
struct RepoListEntry {
    account: String,
    #[serde(flatten)]
    repo: rupu_scm::Repo,
}

pub async fn dispatch_list(args: Value, reg: &Registry) -> Result<String, McpError> {
    let parsed: ListReposArgs =
        serde_json::from_value(args).map_err(|e| McpError::InvalidArgs(e.to_string()))?;
    let platform = resolve_platform(parsed.platform.as_deref(), reg)?;

    // "Account-scoped, no repo" (spec §6.2): there is no RepoRef to run
    // the rule engine against, so `account` (when given) is a direct
    // lookup rather than an `explicit` tier input, and omitting it fans
    // out over every configured account of `platform` rather than
    // guessing one.
    let accounts: Vec<(AccountId, _)> = match parsed.account.as_deref() {
        Some(name) => {
            let id = AccountId::new(name);
            let conn = reg.repo_by_account(&id).ok_or_else(|| {
                McpError::InvalidArgs(format!("no such account: {}", id.as_str()))
            })?;
            vec![(id, conn)]
        }
        None => reg.all_repo_connectors(platform),
    };
    if accounts.is_empty() {
        return Err(McpError::NotWiredInV0(format!(
            "no connector for {platform}"
        )));
    }

    let mut entries = Vec::new();
    for (account, conn) in accounts {
        let repos = conn.list_repos().await?;
        entries.extend(repos.into_iter().map(|repo| RepoListEntry {
            account: account.to_string(),
            repo,
        }));
    }
    Ok(serde_json::to_string(&entries).unwrap())
}

pub async fn dispatch_get(args: Value, reg: &Registry) -> Result<String, McpError> {
    let parsed: GetRepoArgs =
        serde_json::from_value(args).map_err(|e| McpError::InvalidArgs(e.to_string()))?;
    let platform = resolve_platform(parsed.platform.as_deref(), reg)?;
    let r = RepoRef {
        platform,
        owner: parsed.owner,
        repo: parsed.repo,
    };
    let account = parsed.account.as_deref().map(AccountId::new);
    let (_account, conn) = reg.repo_for(&r, None, account.as_ref())?;
    Ok(serde_json::to_string(&conn.get_repo(&r).await?).unwrap())
}

/// Resolve an optional platform argument. If `arg` is `Some`, parse it;
/// otherwise fall back to `Registry::default_platform()` — the first
/// registered platform that has any account with a live connector (not
/// merely "the first `Platform` variant"), honoring `[scm.default]` when
/// configured. See `default_platform`'s own doc for the exact tiers.
pub(crate) fn resolve_platform(arg: Option<&str>, reg: &Registry) -> Result<Platform, McpError> {
    match arg {
        Some(s) => s.parse::<Platform>().map_err(McpError::InvalidArgs),
        None => reg.default_platform().ok_or_else(|| {
            McpError::InvalidArgs("no platform arg and no [scm.default] configured".into())
        }),
    }
}
