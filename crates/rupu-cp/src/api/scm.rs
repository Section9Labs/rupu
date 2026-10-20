//! SCM accounts and account-selection rules — what `rupu scm accounts` and
//! `rupu scm bind` show and write:
//!
//! - `GET /api/scm/accounts`: the configured github/gitlab accounts, the
//!   rules that select each and whether a credential is stored for it.
//! - `GET /api/scm/rules`: the global config's `[[scm.rules]]`.
//! - `POST /api/scm/rules`: append one rule (`rupu scm bind`).
//!
//! All three read and write the GLOBAL config (`rupu scm bind` writes
//! there; the CP has no working directory to find a project by) and are
//! local-only. No response carries any part of a credential.

use crate::{
    api::config::require_writable_to,
    config_write::{append_scm_rule, ConfigWriteError},
    error::{ApiError, ApiResult},
    state::AppState,
};
use axum::{
    extract::{Query, State},
    routing::get,
    Json, Router,
};
use rupu_runtime::account_status::{self, AuthState, ScmAccount};
use serde::{Deserialize, Serialize};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/scm/accounts", get(list_accounts))
        .route("/api/scm/rules", get(list_rules).post(bind_rule))
}

/// `?host=` on an endpoint only the local host serves.
#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub(crate) struct LocalHostQuery {
    /// `local` or omitted; any other host is refused with `HTTP 501`.
    pub(crate) host: Option<String>,
}

/// Refuse a remote `host`: `what` is served from this machine's own
/// config, credential store or repo registry only.
pub(crate) fn local_only(host: Option<&str>, what: &str) -> ApiResult<()> {
    match host {
        None | Some("") | Some("local") => Ok(()),
        Some(h) => Err(ApiError::not_available(format!(
            "{what} is local-only: host `{h}` can't be asked for it"
        ))),
    }
}

/// The global config as it is on disk now: a `rupu scm bind` from the CLI
/// lands without the CP reloading.
pub(crate) fn global_config(s: &AppState) -> rupu_config::Config {
    AppState::resolve_global_config(&s.global_dir)
}

/// One `GET /api/scm/accounts` row.
#[derive(Debug, Serialize)]
struct ScmAccountDto {
    #[serde(flatten)]
    account: ScmAccount,
    /// A stored API key or SSO token, or `RUPU_<ACCOUNT>_API_KEY` set.
    credentials: bool,
    /// `ok`, `needs_login` (only an expired SSO token) or `absent`.
    state: AuthState,
}

async fn list_accounts(
    State(s): State<AppState>,
    Query(q): Query<LocalHostQuery>,
) -> ApiResult<Json<Vec<ScmAccountDto>>> {
    local_only(q.host.as_deref(), "listing SCM accounts")?;
    let cfg = global_config(&s);
    let resolver = rupu_auth::KeychainResolver::for_home(&s.global_dir);
    let mut out = Vec::new();
    for account in account_status::scm_accounts(&cfg) {
        let auth = account_status::account_auth(&cfg, &resolver, &account.account).await;
        out.push(ScmAccountDto {
            credentials: auth.state != AuthState::Absent,
            state: auth.state,
            account,
        });
    }
    Ok(Json(out))
}

async fn list_rules(
    State(s): State<AppState>,
    Query(q): Query<LocalHostQuery>,
) -> ApiResult<Json<Vec<rupu_config::ScmRule>>> {
    local_only(q.host.as_deref(), "listing SCM rules")?;
    Ok(Json(global_config(&s).scm.rules))
}

/// `POST /api/scm/rules`: give exactly one of `owner` or `path`.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct BindBody {
    /// Owner glob the rule matches (`acme/*` or `acme`), against a repo's owner.
    owner: Option<String>,
    /// Filesystem path glob the rule matches (`~/Code/work/*`), against a run's working directory.
    path: Option<String>,
    /// The account the rule selects: a `[scm.<name>]` table or `github` / `gitlab`.
    account: String,
}

#[derive(Debug, Serialize)]
struct BindReply {
    rule: rupu_config::ScmRule,
    /// Set when `account` is neither a declared `[scm.*]` account nor a
    /// bare vendor name: the rule is written but may never match.
    #[serde(skip_serializing_if = "Option::is_none")]
    warning: Option<String>,
    /// Every rule after the write, in config order.
    rules: Vec<rupu_config::ScmRule>,
}

async fn bind_rule(
    State(s): State<AppState>,
    Query(q): Query<LocalHostQuery>,
    Json(body): Json<BindBody>,
) -> ApiResult<Json<BindReply>> {
    local_only(q.host.as_deref(), "binding an SCM rule")?;
    require_writable_to(&s, "binding an SCM rule")?;
    let path = s.global_dir.join("config.toml");
    let rule = tokio::task::spawn_blocking(move || {
        append_scm_rule(
            &path,
            body.owner.as_deref(),
            body.path.as_deref(),
            &body.account,
        )
    })
    .await
    .map_err(|e| ApiError::internal(format!("bind task: {e}")))?
    .map_err(|e| match e {
        ConfigWriteError::Validate(m) => ApiError::bad_request(m),
        ConfigWriteError::Locked(m) => ApiError::conflict(format!("config busy: {m}")),
        ConfigWriteError::Io(m) => ApiError::internal(m),
    })?;
    s.reload_config();
    let cfg = global_config(&s);
    let warning = (!account_status::is_known_scm_account(&cfg, &rule.account)).then(|| {
        format!(
            "`{}` is not a declared [scm.*] account or a bare vendor name (github/gitlab): \
             this rule may never match until one exists",
            rule.account
        )
    });
    Ok(Json(BindReply {
        rule,
        warning,
        rules: cfg.scm.rules,
    }))
}
