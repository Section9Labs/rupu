//! Engagement profiles: the built-ins plus every `*.toml` profile the run
//! assembler would overlay on them (`<global>/profiles`, then, with
//! `?ws_id=`, that project's `.rupu/profiles`; a later layer overrides an
//! earlier one by id). Local only.
//!
//! - `GET /api/engagement-profiles[?ws_id=]` — every profile, each with its
//!   layer, bundle, asset kinds and completeness checks.
//! - `GET /api/engagement-profiles/:id[?ws_id=]` — one of them.
//!
//! A launch fails closed when any discovered profile file doesn't load
//! (`rupu_coverage::registry_with_overlay`), so the response carries those
//! errors rather than hiding them.

use crate::error::{ApiError, ApiResult};
use crate::state::AppState;
use axum::{
    extract::{Path, Query, State},
    routing::get,
    Json, Router,
};
use rupu_coverage::profile::discover;
use rupu_coverage::{EngagementProfile, ProfileRegistry};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/engagement-profiles", get(list_profiles))
        .route("/api/engagement-profiles/:id", get(get_profile))
}

#[derive(Deserialize, Default, schemars::JsonSchema)]
pub(crate) struct ProfilesQuery {
    /// A project id: overlay that project's `.rupu/profiles` too.
    #[serde(default)]
    ws_id: Option<String>,
    /// Must be this host: the listing is local-only.
    #[serde(default)]
    host: Option<String>,
}

#[derive(Serialize)]
struct ProfileRow {
    /// `builtin`, `global` or `project`: the layer this id resolves from.
    layer: &'static str,
    #[serde(flatten)]
    profile: EngagementProfile,
}

struct Listing {
    profiles: BTreeMap<String, ProfileRow>,
    /// Why the overlay doesn't load: every engagement launch fails with it.
    errors: Vec<String>,
}

/// The profile directories of each overlay layer, in override order.
fn overlay_dirs(s: &AppState, ws_id: Option<&str>) -> ApiResult<Vec<(&'static str, PathBuf)>> {
    let mut dirs = vec![("global", s.global_dir.join("profiles"))];
    if let Some(id) = ws_id {
        let w = crate::api::code::load_workspace(s, id)?;
        dirs.push((
            "project",
            PathBuf::from(&w.path).join(".rupu").join("profiles"),
        ));
    }
    Ok(dirs)
}

fn listing(dirs: Vec<(&'static str, PathBuf)>) -> Listing {
    let mut profiles: BTreeMap<String, ProfileRow> = rupu_coverage::builtin_profiles()
        .into_iter()
        .map(|(id, profile)| {
            (
                id,
                ProfileRow {
                    layer: "builtin",
                    profile,
                },
            )
        })
        .collect();
    let mut errors = Vec::new();
    for (layer, dir) in dirs {
        let (found, errs) = discover(&[dir]);
        errors.extend(errs);
        for (id, profile) in found {
            profiles.insert(id, ProfileRow { layer, profile });
        }
    }
    errors.sort();
    if errors.is_empty() {
        let raw = profiles
            .iter()
            .map(|(id, r)| (id.clone(), r.profile.clone()))
            .collect();
        if let Err(e) = ProfileRegistry::from_profiles(raw) {
            errors.push(e.to_string());
        }
    }
    Listing { profiles, errors }
}

async fn load(s: &AppState, q: &ProfilesQuery) -> ApiResult<Listing> {
    crate::api::fleet::require_local(q.host.as_deref(), "listing engagement profiles")?;
    let dirs = overlay_dirs(s, q.ws_id.as_deref())?;
    tokio::task::spawn_blocking(move || listing(dirs))
        .await
        .map_err(|e| ApiError::internal(e.to_string()))
}

/// `GET /api/engagement-profiles[?ws_id=]` → `{profiles, errors}`.
async fn list_profiles(
    State(s): State<AppState>,
    Query(q): Query<ProfilesQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    let l = load(&s, &q).await?;
    let profiles: Vec<&ProfileRow> = l.profiles.values().collect();
    Ok(Json(serde_json::json!({
        "profiles": profiles,
        "errors": l.errors,
    })))
}

/// `GET /api/engagement-profiles/:id[?ws_id=]` → the profile, plus the
/// overlay's `errors`; 404 for an unknown id.
async fn get_profile(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<ProfilesQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    let mut l = load(&s, &q).await?;
    let row = l
        .profiles
        .remove(&id)
        .ok_or_else(|| ApiError::not_found(format!("engagement profile {id} not found")))?;
    let mut v = serde_json::to_value(&row).map_err(|e| ApiError::internal(e.to_string()))?;
    v["errors"] = serde_json::json!(l.errors);
    Ok(Json(v))
}
