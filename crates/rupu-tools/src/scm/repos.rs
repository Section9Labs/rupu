//! `scm.repos.{list, get}`.

use crate::connector::{self, json, parse, resolve_platform, schema, ConnectorError};
use crate::descriptor::{Effect, Service, ToolDescriptor};
use crate::tool::{Tool, ToolContext, ToolError, ToolOutput};
use async_trait::async_trait;
use rupu_scm::{AccountError, AccountId, Platform, Registry, RepoRef};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;

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

pub static LIST: ToolDescriptor = ToolDescriptor {
    name: "scm.repos.list",
    aliases: &[],
    effect: Effect::Read,
    needs: &[Service::Scm],
    uses: &[],
    description: "List repositories the authenticated user can access on the given platform. Omit `platform` to use [scm.default].",
    input_schema: schema::<ListReposArgs>,
};

pub static GET: ToolDescriptor = ToolDescriptor {
    name: "scm.repos.get",
    aliases: &[],
    effect: Effect::Read,
    needs: &[Service::Scm],
    uses: &[],
    description: "Fetch a single repository (default branch, clone URLs, visibility, description).",
    input_schema: schema::<GetRepoArgs>,
};

pub struct ReposListTool;

#[async_trait]
impl Tool for ReposListTool {
    fn descriptor(&self) -> &'static ToolDescriptor {
        &LIST
    }

    async fn invoke(&self, input: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        connector::invoke(ctx, |reg| list(input, reg)).await
    }
}

pub struct ReposGetTool;

#[async_trait]
impl Tool for ReposGetTool {
    fn descriptor(&self) -> &'static ToolDescriptor {
        &GET
    }

    async fn invoke(&self, input: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        connector::invoke(ctx, |reg| get(input, reg)).await
    }
}

/// A row of `scm.repos.list`'s response: the repo plus which configured
/// account it came from. `account` is additive relative to the bare
/// `Repo` shape — every field of `Repo` is still present, flattened, so an
/// agent that only reads `default_branch`/`clone_url_https`/etc. is
/// unaffected; only an agent that needs to disambiguate two accounts of the
/// same platform (e.g. to pass `account` back into `scm.repos.get`) needs to
/// read the new field.
#[derive(serde::Serialize)]
struct RepoListEntry {
    account: String,
    #[serde(flatten)]
    repo: rupu_scm::Repo,
}

async fn list(args: Value, reg: &Registry) -> Result<String, ConnectorError> {
    let parsed: ListReposArgs = parse(args)?;

    // `platform` is parsed (not defaulted) up front so the `account`
    // branch below can check it against the resolved account's real
    // kind without forcing a `[scm.default]` lookup when `account` alone
    // already determines the connector.
    let platform_arg = parsed
        .platform
        .as_deref()
        .map(|s| s.parse::<Platform>().map_err(ConnectorError::InvalidArgs))
        .transpose()?;

    // "Account-scoped, no repo" (spec §6.2): there is no RepoRef to run
    // the rule engine against, so `account` (when given) is a direct
    // lookup rather than an `explicit` tier input, and omitting it fans
    // out over every configured account of `platform` rather than
    // guessing one.
    let accounts: Vec<(AccountId, _)> = match parsed.account.as_deref() {
        Some(name) => {
            let id = AccountId::new(name);
            let conn = match reg.repo_by_account(&id) {
                Some(conn) => conn,
                None => {
                    // `repo_by_account` is a bare lookup with no kind
                    // filtering, so the candidate list for an unknown
                    // name is "every repo account of the requested
                    // platform" when one was given, else the union
                    // across both platforms — mirroring the candidate
                    // lists `repo_for`'s own `UnknownAccount` uses.
                    let mut configured = match platform_arg {
                        Some(p) => reg.accounts_for(p),
                        None => {
                            let mut c = reg.accounts_for(Platform::Github);
                            c.extend(reg.accounts_for(Platform::Gitlab));
                            c
                        }
                    };
                    configured.sort();
                    return Err(AccountError::UnknownAccount {
                        requested: id,
                        configured,
                    }
                    .into());
                }
            };
            // `repo_by_account` does no kind filtering (unlike
            // `repo_for`'s `lookup_repo`), so an `account` of the wrong
            // vendor would otherwise silently list GitLab repos under a
            // `{"platform":"github"}` request — self-describing per row
            // (each carries its own `r.platform`), but still a request
            // that claimed one platform and got another.
            if let Some(requested) = platform_arg {
                let actual = conn.platform();
                if actual != requested {
                    return Err(ConnectorError::InvalidArgs(format!(
                        "account '{id}' is a {actual} account, not {requested}"
                    )));
                }
            }
            vec![(id, conn)]
        }
        None => {
            let platform = match platform_arg {
                Some(p) => p,
                None => resolve_platform(None, reg)?,
            };
            let accounts = reg.all_repo_connectors(platform);
            if accounts.is_empty() {
                return Err(AccountError::NoAccounts {
                    platform: platform.to_string(),
                }
                .into());
            }
            accounts
        }
    };

    // Warn-and-continue per account, not `?`: the fan-out must not be
    // all-or-nothing. One account with an expired token would otherwise
    // abort the whole tool call and lose every OTHER account's repos —
    // strictly worse for an agent than for a human at a terminal, since
    // an agent has no partial result on screen to retry against.
    // Mirrors `rupu-cli`'s sibling fan-outs (`cmd/repos.rs`,
    // `cp_repos.rs`). Errors out only if every account failed — a total
    // outage should still surface rather than returning a silently
    // empty list.
    let total = accounts.len();
    let mut failures = Vec::new();
    let mut entries = Vec::new();
    for (account, conn) in accounts {
        match conn.list_repos().await {
            Ok(repos) => {
                entries.extend(repos.into_iter().map(|repo| RepoListEntry {
                    account: account.to_string(),
                    repo,
                }));
            }
            Err(e) => {
                tracing::warn!(
                    account = %account,
                    error = %e,
                    "scm.repos.list: list_repos failed; skipping account"
                );
                failures.push(format!("{account}: {e}"));
            }
        }
    }
    if !failures.is_empty() && failures.len() == total {
        return Err(ConnectorError::Dispatch(rupu_scm::ScmError::Transient(
            anyhow::anyhow!("every configured account failed: {}", failures.join("; ")),
        )));
    }
    Ok(json(&entries))
}

async fn get(args: Value, reg: &Registry) -> Result<String, ConnectorError> {
    let parsed: GetRepoArgs = parse(args)?;
    let platform = resolve_platform(parsed.platform.as_deref(), reg)?;
    let r = RepoRef {
        platform,
        owner: parsed.owner,
        repo: parsed.repo,
    };
    let account = parsed.account.as_deref().map(AccountId::new);
    let (_account, conn) = reg.repo_for(&r, None, account.as_ref())?;
    Ok(json(&conn.get_repo(&r).await?))
}
