//! `GET /api/auth/status` — what `rupu auth status` shows: every provider
//! and SCM account the global config declares (plus the built-in vendor
//! names), its kind, whether an API key or SSO token is stored, the SSO
//! token's expiry and whether the account needs a login. Local-only, and
//! never any part of a credential. Logging in stays a human step
//! (`rupu auth login`).

use crate::{
    api::scm::{global_config, local_only, LocalHostQuery},
    error::ApiResult,
    state::AppState,
};
use axum::{
    extract::{Query, State},
    routing::get,
    Json, Router,
};
use rupu_runtime::account_status::{self, AccountAuth};

pub fn routes() -> Router<AppState> {
    Router::new().route("/api/auth/status", get(status))
}

async fn status(
    State(s): State<AppState>,
    Query(q): Query<LocalHostQuery>,
) -> ApiResult<Json<Vec<AccountAuth>>> {
    local_only(q.host.as_deref(), "auth status")?;
    let cfg = global_config(&s);
    let resolver = rupu_auth::KeychainResolver::for_home(&s.global_dir);
    Ok(Json(account_status::auth_status(&cfg, &resolver).await))
}
