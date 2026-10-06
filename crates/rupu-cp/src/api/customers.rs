//! `/api/customers*` — customer CRUD, project assignment, the list with
//! rollups, and the detail page.
//!
//! Spec: `docs/superpowers/plans/2026-10-06-rupu-customers-plan-2a-cp-backend.md`
//! (Task 5; rulings 4, 8, 9).
//!
//! Rollups (list and detail) are computed in ONE blocking pass per request
//! for every customer the response covers: the run store is listed once,
//! each run is attributed by its recorded customer else its workspace's
//! CURRENT assignment ([`crate::customers::CustomerLookup`], memoized per
//! workspace), and priced with that customer's pricing (global + customer
//! layer, [`crate::customers::CustomerPricing`]). Standalone agent runs and
//! session turns add spend and activity, never `run_count` — the same rule
//! as the Projects page. `projects` and `findings_open` follow the current
//! assignment (findings carry no recorded customer, ruling 6).

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path as FsPath;

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post, put},
    Json, Router,
};
use chrono::{DateTime, Utc};
use rupu_orchestrator::runs::RunStore;
use rupu_workspace::{
    Customer, CustomerError, CustomerStore, MetaPatch, NewCustomer, ProjectRef, Workspace,
    WorkspaceStore,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::api::config::require_writable_to;
use crate::api::projects::{apply_rollup, project_rollups, project_row, ProjectRow};
use crate::api::runs::blocking;
pub use crate::customers::DefaultAccount;
use crate::customers::{customer_dto, CustomerDto, CustomerLookup, PricingMemo, Recorded};
use crate::error::{ApiError, ApiResult};
use crate::host::dashboard_summary::DashboardRange;
use crate::state::AppState;
use crate::usage::{EntityRollup, UsageSummary};

#[derive(Serialize)]
pub struct CustomerRow {
    #[serde(flatten)]
    pub customer: CustomerDto,
    pub rollup: CustomerRollup,
    pub default_account: Option<DefaultAccount>,
    /// Why the customer's config could not be resolved (a malformed layer):
    /// `default_account` is then `null` and its work is priced at the global
    /// rates (`rollup.usage.pricing_error` says so too). `null` otherwise.
    pub layer_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct CustomerRollup {
    /// Projects currently assigned to the customer.
    pub projects: u64,
    /// Workflow runs attributed to the customer in the range.
    pub run_count: u64,
    /// Workflow runs + standalone agent runs + session turns in the range,
    /// priced with the customer's pricing.
    pub usage: UsageSummary,
    /// Findings in the customer's current projects' coverage ledgers.
    pub findings_open: u64,
    pub last_active: Option<String>,
}

#[derive(Serialize)]
pub struct CustomerDetail {
    pub customer: CustomerDto,
    pub rollup: CustomerRollup,
    /// The customer's current projects, with the Projects page's own
    /// (all-time) rollups, each run priced with the pricing of the customer
    /// it recorded.
    pub projects: Vec<ProjectRow>,
    pub default_account: Option<DefaultAccount>,
    /// Why the customer's config could not be resolved (a malformed layer);
    /// `default_account` is then `None`.
    pub layer_error: Option<String>,
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/customers", get(list_customers).post(create_customer))
        .route(
            "/api/customers/:slug",
            get(get_customer)
                .patch(patch_customer)
                .delete(delete_customer),
        )
        .route("/api/customers/:slug/archive", post(archive_customer))
        .route("/api/customers/:slug/unarchive", post(unarchive_customer))
        .route(
            "/api/customers/:slug/projects/:ws_id",
            put(assign_project).delete(unassign_project),
        )
}

/// Map a store error to its HTTP status. `HasProjects` is answered by
/// [`delete_customer`] itself, with the projects in the body.
pub(crate) fn api_err(e: CustomerError) -> ApiError {
    let status = match &e {
        CustomerError::InvalidSlug(_)
        | CustomerError::ReservedSlug(_)
        | CustomerError::InvalidColor(_)
        | CustomerError::EmptyName
        | CustomerError::InvalidWsId(_) => StatusCode::BAD_REQUEST,
        CustomerError::Exists(_)
        | CustomerError::Archived(_)
        | CustomerError::HasProjects { .. } => StatusCode::CONFLICT,
        CustomerError::NotFound(_) | CustomerError::NoProject(_) => StatusCode::NOT_FOUND,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };
    ApiError(status, e.to_string())
}

/// Customer writes need a `cp serve` deployment (501 otherwise).
fn require_writable(s: &AppState) -> ApiResult<()> {
    require_writable_to(s, "managing customers")
}

fn customers(s: &AppState) -> CustomerStore {
    CustomerStore::new(s.global_dir.clone())
}

#[derive(Debug, Deserialize, Default)]
struct RangeQuery {
    archived: Option<String>,
    range: Option<String>,
}

impl RangeQuery {
    fn range(&self) -> ApiResult<DashboardRange> {
        match self.range.as_deref() {
            None => Ok(DashboardRange::default()),
            Some(r) => DashboardRange::parse(r).ok_or_else(|| {
                ApiError::bad_request(format!("range: expected 7d, 30d or all, got `{r}`"))
            }),
        }
    }

    fn include_archived(&self) -> bool {
        matches!(self.archived.as_deref(), Some("1" | "true"))
    }
}

/// Everything one request's rollups are computed from, read once.
struct Pass {
    run_store: std::sync::Arc<RunStore>,
    runs: Vec<rupu_orchestrator::RunRecord>,
    extras: Vec<crate::usage_sources::ExtraSource>,
    workspaces: Vec<Workspace>,
    lookup: CustomerLookup,
}

impl Pass {
    /// Blocking IO. A run store or workspace registry that cannot be read
    /// fails the request (500): a rollup over what could be read would
    /// report a customer's work as smaller than it is.
    fn read(s_global: &FsPath, run_store: std::sync::Arc<RunStore>) -> ApiResult<Self> {
        let runs = run_store
            .list()
            .map_err(|e| ApiError::internal(format!("cannot list runs: {e}")))?;
        let claimed = crate::usage_sources::claimed_transcripts(&run_store, &runs);
        let extras = crate::usage_sources::extra_sources(s_global, &run_store, &claimed);
        let workspaces = WorkspaceStore {
            root: s_global.join("workspaces"),
        }
        .list()
        .map_err(|e| ApiError::internal(format!("cannot list projects: {e}")))?;
        Ok(Self {
            run_store,
            runs,
            extras,
            workspaces,
            lookup: CustomerLookup::new(CustomerStore::new(s_global)),
        })
    }

    /// Each customer's current projects (by the memoized assignment).
    fn projects_by_customer(&mut self) -> ApiResult<BTreeMap<String, Vec<Workspace>>> {
        let mut out: BTreeMap<String, Vec<Workspace>> = BTreeMap::new();
        for w in &self.workspaces {
            if let Some(slug) = self.lookup.assigned(&w.id)? {
                out.entry(slug).or_default().push(w.clone());
            }
        }
        for list in out.values_mut() {
            list.sort_by(|a, b| a.path.cmp(&b.path));
        }
        Ok(out)
    }

    /// Rollups for each slug in `wanted`, over runs started since `since`,
    /// each priced with its customer's pricing.
    fn rollups(
        &mut self,
        wanted: &BTreeSet<String>,
        since: Option<DateTime<Utc>>,
        prices: &mut PricingMemo<'_>,
    ) -> ApiResult<BTreeMap<String, CustomerRollup>> {
        let in_range = |at: Option<DateTime<Utc>>| match since {
            None => true,
            Some(cut) => at.is_some_and(|t| t >= cut),
        };
        let mut rolls: BTreeMap<String, EntityRollup> = BTreeMap::new();

        for run in &self.runs {
            if !in_range(Some(run.started_at)) {
                continue;
            }
            let who = self
                .lookup
                .attribute(Recorded::of(&run.customer), &run.workspace_id)?;
            let Some(slug) = who.slug.filter(|s| wanted.contains(s)) else {
                continue;
            };
            let usage =
                crate::usage::summarize_run(&self.run_store, &run.id, prices.get(Some(&slug)));
            let usage = prices.stamp(usage, Some(&slug));
            rolls
                .entry(slug)
                .or_default()
                .add(&usage, Some(run.started_at.to_rfc3339()));
        }

        for src in &self.extras {
            if !in_range(src.started_at) {
                continue;
            }
            let who = src.attribute(&mut self.lookup)?;
            let Some(slug) = who.slug.filter(|s| wanted.contains(s)) else {
                continue;
            };
            let usage = crate::usage::summarize_run_usage(
                &crate::usage::transcripts_usage(&src.paths),
                prices.get(Some(&slug)),
            );
            let usage = prices.stamp(usage, Some(&slug));
            rolls
                .entry(slug)
                .or_default()
                .add_spend(&usage, src.started_at.map(|t| t.to_rfc3339()));
        }

        let projects = self.projects_by_customer()?;
        let assigned: Vec<Workspace> = projects
            .iter()
            .filter(|(slug, _)| wanted.contains(*slug))
            .flat_map(|(_, ws)| ws.iter().cloned())
            .collect();
        let findings = crate::api::findings::count_findings_by_workspace(&assigned);

        Ok(wanted
            .iter()
            .map(|slug| {
                let roll = rolls.remove(slug).unwrap_or_default();
                let mine = projects.get(slug).map(Vec::as_slice).unwrap_or_default();
                let rollup = CustomerRollup {
                    projects: mine.len() as u64,
                    run_count: roll.run_count,
                    usage: roll.usage,
                    findings_open: mine
                        .iter()
                        .map(|w| findings.get(&w.id).copied().unwrap_or(0))
                        .sum(),
                    last_active: roll.last_active,
                };
                (slug.clone(), rollup)
            })
            .collect())
    }
}

/// `GET /api/customers?archived=1&range=7d|30d|all` — every customer
/// (archived ones only with `archived=1`) with its rollups over the range
/// (default `30d`) and its default account. A customer whose config layer
/// does not resolve is still listed, with `default_account: null` and its
/// `layer_error`, its work priced at the global rates. An
/// assignment that cannot be read fails the listing (500 naming the
/// workspace), never counts as "no customer".
async fn list_customers(
    State(s): State<AppState>,
    Query(q): Query<RangeQuery>,
) -> ApiResult<Json<Vec<CustomerRow>>> {
    let since = q.range()?.since(Utc::now());
    let include_archived = q.include_archived();
    let global = s.global_dir.clone();
    let run_store = std::sync::Arc::clone(&s.run_store);
    let pricing = std::sync::Arc::clone(&s.customer_pricing);
    let rows = blocking(move || {
        let store = CustomerStore::new(global.clone());
        let list = store.list(include_archived).map_err(api_err)?;
        let wanted: BTreeSet<String> = list.iter().map(|c| c.slug.clone()).collect();
        let mut prices = PricingMemo::new(&pricing);
        let mut pass = Pass::read(&global, run_store)?;
        let mut rollups = pass.rollups(&wanted, since, &mut prices)?;
        Ok(list
            .iter()
            .map(|c| {
                let (default_account, layer_error) = match pricing.default_account(&c.slug) {
                    Ok(a) => (a, None),
                    Err(e) => {
                        tracing::debug!(customer = %c.slug, error = %e, "customer config does not resolve");
                        (None, Some(e))
                    }
                };
                CustomerRow {
                    customer: customer_dto(c),
                    rollup: rollups.remove(&c.slug).unwrap_or_default(),
                    default_account,
                    layer_error,
                }
            })
            .collect())
    })
    .await?;
    Ok(Json(rows))
}

/// `GET /api/customers/:slug?range=` — one customer (archived included):
/// its rollups over the range, its current projects (with the Projects
/// page's all-time rollups, priced per run's customer), its default account
/// and, when its config layer does not resolve, why.
async fn get_customer(
    State(s): State<AppState>,
    Path(slug): Path<String>,
    Query(q): Query<RangeQuery>,
) -> ApiResult<Json<CustomerDetail>> {
    let since = q.range()?.since(Utc::now());
    let global = s.global_dir.clone();
    let run_store = std::sync::Arc::clone(&s.run_store);
    let pricing = std::sync::Arc::clone(&s.customer_pricing);
    let detail = blocking(move || {
        let store = CustomerStore::new(global.clone());
        let c = store.get(&slug).map_err(api_err)?;
        let mut prices = PricingMemo::new(&pricing);
        let mut pass = Pass::read(&global, run_store)?;
        let wanted = BTreeSet::from([slug.clone()]);
        let rollup = pass
            .rollups(&wanted, since, &mut prices)?
            .remove(&slug)
            .unwrap_or_default();
        let mine = pass
            .projects_by_customer()?
            .remove(&slug)
            .unwrap_or_default();
        let ids: BTreeSet<&str> = mine.iter().map(|w| w.id.as_str()).collect();
        let project_rolls = project_rollups(
            &pass.run_store,
            &pass.runs,
            &pass.extras,
            &mut prices,
            &mut pass.lookup,
            |w| ids.contains(w),
        )?;
        let mut projects = Vec::with_capacity(mine.len());
        for w in &mine {
            let mut row = project_row(w);
            if let Some(roll) = project_rolls.get(&w.id) {
                apply_rollup(&mut row, roll);
            }
            row.customer = pass.lookup.project_customer(&w.id)?;
            projects.push(row);
        }
        let (default_account, layer_error) = match pricing.default_account(&slug) {
            Ok(a) => (a, None),
            Err(e) => (None, Some(e)),
        };
        Ok(CustomerDetail {
            customer: customer_dto(&c),
            rollup,
            projects,
            default_account,
            layer_error,
        })
    })
    .await?;
    Ok(Json(detail))
}

#[derive(Debug, Deserialize)]
struct CreateBody {
    slug: String,
    name: String,
    notes: Option<String>,
    contact: Option<String>,
    color: Option<String>,
}

/// `POST /api/customers` — `201` with the new customer.
async fn create_customer(
    State(s): State<AppState>,
    Json(body): Json<CreateBody>,
) -> ApiResult<(StatusCode, Json<CustomerDto>)> {
    require_writable(&s)?;
    let store = customers(&s);
    let c = blocking(move || {
        store
            .create(
                &body.slug,
                &NewCustomer {
                    name: body.name,
                    notes: body.notes,
                    contact: body.contact,
                    color: body.color,
                },
            )
            .map_err(api_err)
    })
    .await?;
    Ok((StatusCode::CREATED, Json(customer_dto(&c))))
}

#[derive(Debug, Deserialize)]
struct PatchBody {
    name: Option<String>,
    notes: Option<String>,
    contact: Option<String>,
    color: Option<String>,
}

/// `PATCH /api/customers/:slug` — absent fields stay; `""` clears `notes`,
/// `contact` or `color`.
async fn patch_customer(
    State(s): State<AppState>,
    Path(slug): Path<String>,
    Json(body): Json<PatchBody>,
) -> ApiResult<Json<CustomerDto>> {
    require_writable(&s)?;
    let store = customers(&s);
    let c = blocking(move || {
        store
            .update_meta(
                &slug,
                &MetaPatch {
                    name: body.name,
                    notes: body.notes,
                    contact: body.contact,
                    color: body.color,
                },
            )
            .map_err(api_err)
    })
    .await?;
    Ok(Json(customer_dto(&c)))
}

async fn set_archived(s: AppState, slug: String, archived: bool) -> ApiResult<Json<CustomerDto>> {
    require_writable(&s)?;
    let store = customers(&s);
    let c: Customer =
        blocking(move || store.set_archived(&slug, archived).map_err(api_err)).await?;
    Ok(Json(customer_dto(&c)))
}

/// `POST /api/customers/:slug/archive`.
async fn archive_customer(
    State(s): State<AppState>,
    Path(slug): Path<String>,
) -> ApiResult<Json<CustomerDto>> {
    set_archived(s, slug, true).await
}

/// `POST /api/customers/:slug/unarchive`.
async fn unarchive_customer(
    State(s): State<AppState>,
    Path(slug): Path<String>,
) -> ApiResult<Json<CustomerDto>> {
    set_archived(s, slug, false).await
}

/// `DELETE /api/customers/:slug` — `204`, or `409 {"error", "projects":
/// [{ws_id, path}]}` while projects are still assigned to it.
async fn delete_customer(
    State(s): State<AppState>,
    Path(slug): Path<String>,
) -> ApiResult<Response> {
    require_writable(&s)?;
    let store = customers(&s);
    blocking(move || match store.delete(&slug) {
        Ok(()) => Ok(StatusCode::NO_CONTENT.into_response()),
        Err(e @ CustomerError::HasProjects { .. }) => {
            let projects: Vec<_> = store
                .projects_of(&slug)
                .map_err(api_err)?
                .into_iter()
                .map(|w| json!({ "ws_id": w.id, "path": w.path }))
                .collect();
            Ok((
                StatusCode::CONFLICT,
                Json(json!({ "error": e.to_string(), "projects": projects })),
            )
                .into_response())
        }
        Err(e) => Err(api_err(e)),
    })
    .await
}

/// `PUT /api/customers/:slug/projects/:ws_id` — assign the project (any
/// previous assignment is replaced); answers the project's row.
async fn assign_project(
    State(s): State<AppState>,
    Path((slug, ws_id)): Path<(String, String)>,
) -> ApiResult<Json<ProjectRow>> {
    require_writable(&s)?;
    let global = s.global_dir.clone();
    let run_store = std::sync::Arc::clone(&s.run_store);
    let pricing = std::sync::Arc::clone(&s.customer_pricing);
    let row = blocking(move || {
        let store = CustomerStore::new(global.clone());
        let w = store
            .assign(&slug, ProjectRef::Id(&ws_id))
            .map_err(api_err)?;
        let runs = run_store
            .list()
            .map_err(|e| ApiError::internal(format!("cannot list runs: {e}")))?;
        let extras = crate::usage_sources::unclaimed_extra_sources(&global, &run_store);
        let mut lookup = CustomerLookup::new(store);
        let mut prices = PricingMemo::new(&pricing);
        let rolls = project_rollups(&run_store, &runs, &extras, &mut prices, &mut lookup, |id| {
            id == w.id
        })?;
        let mut row = project_row(&w);
        if let Some(roll) = rolls.get(&w.id) {
            apply_rollup(&mut row, roll);
        }
        row.customer = lookup.project_customer(&w.id)?;
        Ok(row)
    })
    .await?;
    Ok(Json(row))
}

/// `DELETE /api/customers/:slug/projects/:ws_id` — `204`; `404` when the
/// project is not assigned to `:slug`.
async fn unassign_project(
    State(s): State<AppState>,
    Path((slug, ws_id)): Path<(String, String)>,
) -> ApiResult<StatusCode> {
    require_writable(&s)?;
    let store = customers(&s);
    blocking(move || {
        store.get(&slug).map_err(api_err)?;
        // Compare-and-remove under the assignment lock: a reassignment to
        // another customer that lands concurrently is never removed.
        if !store
            .unassign_if(&slug, ProjectRef::Id(&ws_id))
            .map_err(api_err)?
        {
            return Err(ApiError::not_found(format!(
                "project {ws_id} is not assigned to customer `{slug}`"
            )));
        }
        Ok(StatusCode::NO_CONTENT)
    })
    .await
}
