//! Local durable Board work, shared with runtime agents through the Fleet API.
//! HTTP and MCP carry operator actions only; no action dispatches an execution.
#[path = "board_work_error.rs"]
mod error;
#[path = "board_work_owner.rs"]
mod owner;
#[path = "board_work_source.rs"]
mod source;
#[cfg(test)]
#[path = "board_work_tests.rs"]
mod tests;

use crate::{host::messages::MessageRoute, state::AppState};
use axum::{
    extract::{
        rejection::{JsonRejection, QueryRejection},
        DefaultBodyLimit, Path as AxPath, Query, State,
    },
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use error::{Error, Result};
use rupu_fleet::{work_queue::*, Board, Bus, FleetError, OperatorVia};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::Path;

pub(crate) const CAPABILITY: &str = "board.work_queue.v1";
pub(crate) const MUTATION_CAPABILITY: &str = "board.work_queue.mutations.v1";
const CURSOR_DOMAIN: &str = "board.work_queue.v1\0";

#[derive(Clone, Default, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct HostQuery {
    pub host: Option<String>,
}
#[derive(Clone, Default, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ListQuery {
    pub host: Option<String>,
    /// queued, claimed, working, blocked, completed, failed or cancelled.
    pub state: Option<String>,
    pub claimant_run_id: Option<String>,
    /// 1 through 200, default 50; the byte budget may return fewer whole rows.
    pub limit: Option<usize>,
    pub cursor: Option<String>,
}
#[derive(Clone, Default, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct TimelineQuery {
    pub host: Option<String>,
    pub limit: Option<usize>,
    pub cursor: Option<String>,
}
fn required_option<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> std::result::Result<Option<String>, D::Error> {
    Option::<String>::deserialize(d)
}
fn nullable_queue_id(generator: &mut schemars::gen::SchemaGenerator) -> schemars::schema::Schema {
    generator.subschema_for::<Option<String>>()
}
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct PostRequest {
    pub owner_token: String,
    pub request_id: String,
    #[serde(deserialize_with = "required_option")]
    #[schemars(required, schema_with = "nullable_queue_id")]
    pub expected_queue_id: Option<String>,
    pub expected_revision: u64,
    pub operation: PostOperation,
}
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum PostOperation {
    Post { title: String, instructions: String },
}
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct TransitionRequest {
    pub owner_token: String,
    pub request_id: String,
    pub expected_queue_id: String,
    pub expected_revision: u64,
    pub operation: TransitionOperation,
}
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum TransitionOperation {
    Cancel { reason: String },
    Requeue { reason: String },
}

fn query<T>(q: std::result::Result<Query<T>, QueryRejection>) -> Result<T> {
    q.map(|Query(q)| q)
        .map_err(|e| Error::invalid(e.body_text()))
}
fn body<T>(b: std::result::Result<Json<T>, JsonRejection>) -> Result<T> {
    b.map(|Json(b)| b).map_err(|e| {
        if e.status() == StatusCode::PAYLOAD_TOO_LARGE {
            Error::new(
                StatusCode::PAYLOAD_TOO_LARGE,
                "work_capacity",
                "Request exceeds the bounded work envelope",
                "none",
            )
        } else {
            Error::invalid(e.body_text())
        }
    })
}
fn local(host: Option<&str>) -> Result<()> {
    if host.is_some_and(|h| h != "local") {
        return Err(Error::unsupported(
            "Board work is local-only; connect directly to its owner",
        ));
    }
    Ok(())
}
fn limit(value: Option<usize>) -> Result<usize> {
    let n = value.unwrap_or(DEFAULT_PAGE_LIMIT);
    if !(1..=MAX_PAGE_LIMIT).contains(&n) {
        return Err(Error::invalid("limit must be 1..200"));
    }
    Ok(n)
}
fn identifier(id: &str) -> Result<()> {
    if id.is_empty()
        || id.len() > 128
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
    {
        return Err(Error::invalid("Invalid bounded work identifier"));
    }
    Ok(())
}
async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<Json<T>> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|_| Error::internal("Board work read failed"))?
        .map(Json)
}
pub(crate) fn routes() -> Router<AppState> {
    let mut router = Router::new();
    for route in MessageRoute::ALL {
        let base = format!("/api/{}/:id/board/work", route.segment());
        router = router.route(
            &base,
            get(
                move |State(s): State<AppState>,
                      AxPath(id): AxPath<String>,
                      q: std::result::Result<Query<ListQuery>, QueryRejection>| async move {
                    let q = query(q)?;
                    local(q.host.as_deref())?;
                    blocking(move || list(&s.global_dir, route, &id, q, s.launcher.is_some())).await
                },
            )
            .post(
                move |State(s): State<AppState>,
                      AxPath(id): AxPath<String>,
                      q: std::result::Result<Query<HostQuery>, QueryRejection>,
                      _via: crate::mcp::ViaMcp,
                      b: std::result::Result<Json<PostRequest>, JsonRejection>| async move {
                    local(query(q)?.host.as_deref())?;
                    let b = body(b)?;
                    let PostOperation::Post {
                        title,
                        instructions,
                    } = b.operation;
                    apply(
                        s,
                        route,
                        id,
                        b.owner_token,
                        WorkCommand {
                            request_id: b.request_id,
                            expected_queue_id: b.expected_queue_id,
                            expected_revision: b.expected_revision,
                            operation: WorkOperation::Post {
                                title,
                                instructions,
                            },
                        },
                    )
                    .await
                },
            ),
        );
        router = router.route(
            &format!("{base}/:work_id"),
            get(
                move |State(s): State<AppState>,
                      AxPath((id, work_id)): AxPath<(String, String)>,
                      q: std::result::Result<Query<HostQuery>, QueryRejection>| async move {
                    local(query(q)?.host.as_deref())?;
                    identifier(&work_id)?;
                    blocking(move || {
                        detail(&s.global_dir, route, &id, &work_id, s.launcher.is_some())
                    })
                    .await
                },
            ),
        );
        router = router.route(&format!("{base}/:work_id/timeline"), get(
            move |State(s): State<AppState>, AxPath((id, work_id)): AxPath<(String,String)>, q: std::result::Result<Query<TimelineQuery>, QueryRejection>| async move {
                let q=query(q)?; local(q.host.as_deref())?; identifier(&work_id)?;
                blocking(move || timeline(&s.global_dir, route, &id, &work_id, q, s.launcher.is_some())).await
            }
        ));
        router = router.route(&format!("{base}/:work_id/transitions"), post(
            move |State(s): State<AppState>, AxPath((id, work_id)): AxPath<(String,String)>, q: std::result::Result<Query<HostQuery>, QueryRejection>, _via: crate::mcp::ViaMcp, b: std::result::Result<Json<TransitionRequest>, JsonRejection>| async move {
                local(query(q)?.host.as_deref())?; identifier(&work_id)?;
                let b=body(b)?;
                let operation = match b.operation {
                    TransitionOperation::Cancel { reason } => WorkOperation::Cancel { work_id, reason },
                    TransitionOperation::Requeue { reason } => WorkOperation::Requeue { work_id, reason },
                };
                apply(s, route, id, b.owner_token, WorkCommand { request_id:b.request_id, expected_queue_id:Some(b.expected_queue_id), expected_revision:b.expected_revision, operation }).await
            }
        ));
    }
    router.layer(DefaultBodyLimit::max(128 * 1024))
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "endpoint", rename_all = "snake_case", deny_unknown_fields)]
enum Cursor {
    Queue {
        owner: String,
        position: WorkPagePosition,
    },
    Timeline {
        owner: String,
        position: WorkTimelinePosition,
    },
}
fn decode(raw: &str) -> Result<Cursor> {
    crate::api::recorded_cursor::decode(CURSOR_DOMAIN, raw).map_err(|_| Error::cursor())
}
fn encode(cursor: Cursor) -> Result<String> {
    crate::api::recorded_cursor::encode(CURSOR_DOMAIN, &cursor).map_err(|e| Error::internal(e.1))
}
fn list(
    global: &Path,
    route: MessageRoute,
    id: &str,
    q: ListQuery,
    service: bool,
) -> Result<Value> {
    let ctx = owner::resolve(global, route, id, service)?;
    let filters = WorkFilters {
        state: q
            .state
            .map(|s| {
                serde_json::from_value(json!(s)).map_err(|_| Error::invalid("Unknown work state"))
            })
            .transpose()?,
        claimant_run_id: q.claimant_run_id,
    };
    if let Some(id) = &filters.claimant_run_id {
        identifier(id)?;
    }
    let cursor = q
        .cursor
        .as_deref()
        .map(|raw| match decode(raw)? {
            Cursor::Queue { owner, position }
                if owner == ctx.owner.token && position.filters == filters =>
            {
                Ok(position)
            }
            _ => Err(Error::cursor()),
        })
        .transpose()?;
    let page = Board::new(&ctx.root.space).work_list(WorkPageQuery {
        filters,
        limit: limit(q.limit)?,
        cursor,
    })?;
    let mut resolver = source::Resolver::new(&ctx);
    let rows: Vec<_> = page.items.iter().map(|row| resolver.summary(row)).collect();
    let result = bounded(rows.len(), |n| {
        let next = if n < rows.len() {
            let last = &page.items[n - 1];
            Some(WorkPagePosition {
                queue_id: page.snapshot.queue_id.clone().ok_or_else(Error::cursor)?,
                source_revision: page.snapshot.revision,
                filters: page.filters.clone(),
                before_creation_sequence: last.creation_sequence,
            })
        } else {
            page.next.clone()
        };
        let next_cursor = next
            .map(|position| {
                encode(Cursor::Queue {
                    owner: ctx.owner.token.clone(),
                    position,
                })
            })
            .transpose()?;
        Ok(
            json!({"version":1,"owner":ctx.owner,"snapshot":page.snapshot,"source_revision":page.snapshot.revision,
            "filters":page.filters,"ordering":page.order,"items":&rows[..n],
            "counts":{"scope":page.counts.scope,"returned":n,"matching_total":page.counts.matching_total},
            "has_more":n<rows.len() || page.has_more,"next_cursor":next_cursor}),
        )
    })?;
    owner::unchanged(&ctx, route, service)?;
    Ok(result)
}
fn actions(writable: bool, state: WorkState) -> Vec<&'static str> {
    if !writable {
        return vec![];
    }
    let mut actions = vec![];
    if matches!(
        state,
        WorkState::Queued | WorkState::Claimed | WorkState::Working | WorkState::Blocked
    ) {
        actions.push("cancel");
    }
    if state != WorkState::Queued {
        actions.push("requeue");
    }
    actions
}
fn detail(
    global: &Path,
    route: MessageRoute,
    id: &str,
    work_id: &str,
    service: bool,
) -> Result<Value> {
    let ctx = owner::resolve(global, route, id, service)?;
    let board = Board::new(&ctx.root.space);
    let detail = board.work_get_snapshot(work_id)?;
    let snapshot = detail.snapshot;
    let item = detail.item;
    let item_value = source::Resolver::new(&ctx).item(&item);
    let result = bounded(0, |_| {
        Ok(json!({"version":1,"owner":ctx.owner,"snapshot":snapshot,
        "source_revision":item.revision,"item":item_value,"allowed_actions":actions(ctx.owner.writable,item.state)}))
    })?;
    owner::unchanged(&ctx, route, service)?;
    Ok(result)
}
fn timeline(
    global: &Path,
    route: MessageRoute,
    id: &str,
    work_id: &str,
    q: TimelineQuery,
    service: bool,
) -> Result<Value> {
    let ctx = owner::resolve(global, route, id, service)?;
    let cursor = q
        .cursor
        .as_deref()
        .map(|raw| match decode(raw)? {
            Cursor::Timeline { owner, position }
                if owner == ctx.owner.token && position.work_id == work_id =>
            {
                Ok(position)
            }
            _ => Err(Error::cursor()),
        })
        .transpose()?;
    let page = Board::new(&ctx.root.space).work_timeline(WorkTimelineQuery {
        work_id: work_id.into(),
        limit: limit(q.limit)?,
        cursor,
    })?;
    let mut resolver = source::Resolver::new(&ctx);
    let rows: Vec<_> = page
        .events
        .iter()
        .map(|event| {
            let mut value = serde_json::to_value(event).expect("work event is serializable");
            value["source_resolution"] = resolver.actor(&event.actor, event.source.as_ref());
            value
        })
        .collect();
    let result = bounded(rows.len(), |n| {
        let next = if n < rows.len() {
            Some(WorkTimelinePosition {
                queue_id: page.queue_id.clone(),
                work_id: work_id.into(),
                source_item_revision: page.source_item_revision,
                after_event_sequence: page.events[n - 1].sequence,
            })
        } else {
            page.next.clone()
        };
        let next_cursor = next
            .map(|position| {
                encode(Cursor::Timeline {
                    owner: ctx.owner.token.clone(),
                    position,
                })
            })
            .transpose()?;
        Ok(
            json!({"version":1,"owner":ctx.owner,"snapshot":page.snapshot,"queue_id":page.queue_id,"work_id":work_id,
            "source_revision":page.source_item_revision,"filters":{"work_id":work_id},"ordering":page.order,"events":&rows[..n],
            "counts":{"scope":page.counts.scope,"returned":n,"matching_total":page.counts.matching_total},
            "has_more":n<rows.len() || page.has_more,"next_cursor":next_cursor}),
        )
    })?;
    owner::unchanged(&ctx, route, service)?;
    Ok(result)
}
/// Binary search whole-record prefixes. The final serialization includes the
/// source wrappers, envelope and signed next cursor, not merely stored rows.
fn bounded(total: usize, build: impl Fn(usize) -> Result<Value>) -> Result<Value> {
    let capacity = || {
        Error::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "work_capacity",
            "A complete work record exceeds the 1 MiB response budget",
            "none",
        )
    };
    let fits = |value: &Value| {
        serde_json::to_vec(value)
            .map(|v| v.len() <= MAX_PAGE_BYTES)
            .map_err(|e| Error::internal(e.to_string()))
    };
    let full = build(total)?;
    if fits(&full)? {
        return Ok(full);
    }
    if total <= 1 {
        return Err(capacity());
    }
    let mut lo = 1;
    let mut hi = total - 1;
    let mut best = None;
    while lo <= hi {
        let mid = lo + (hi - lo) / 2;
        let value = build(mid)?;
        if fits(&value)? {
            best = Some(value);
            lo = mid + 1;
        } else {
            hi = mid - 1;
        }
    }
    best.ok_or_else(capacity)
}
async fn apply(
    state: AppState,
    route: MessageRoute,
    id: String,
    token: String,
    command: WorkCommand,
) -> Result<Json<Value>> {
    if state.launcher.is_none() {
        return Err(Error::unsupported(
            "Board work mutation service is not installed",
        ));
    }
    let request_id = command.request_id.clone();
    let task_request = request_id.clone();
    tokio::task::spawn_blocking(move || mutate(&state.global_dir, route, &id, &token, command))
        .await
        .map_err(|_| {
            Error::reconciliation(
                &task_request,
                "Board work writer stopped; replay the identical request",
            )
        })?
        .map(Json)
        .map_err(|e| e.with_request(&request_id))
}
fn mutate(
    global: &Path,
    route: MessageRoute,
    id: &str,
    token: &str,
    command: WorkCommand,
) -> Result<Value> {
    // The same outer lease guards session archive. Holding it through commit
    // prevents a successful write into a directory renamed out from under Bus.
    let _lease =
        rupu_runtime::retention_lease::RetentionLease::for_legacy(global).map_err(|e| {
            if e.kind() == std::io::ErrorKind::WouldBlock {
                Error::from(WorkError::Busy)
            } else {
                Error::internal(format!("Board lifecycle lease unavailable: {e}"))
            }
        })?;
    let ctx = owner::resolve(global, route, id, true)?;
    if token != ctx.owner.token || ctx.owner.archived {
        return Err(Error::owner_changed());
    }
    let request_id = command.request_id.clone();
    let check_owner = || {
        let current = owner::resolve(global, route, id, true)
            .map_err(|_| FleetError::Refused("Board owner changed".into()))?;
        if current.owner.token != ctx.owner.token || current.owner.archived {
            return Err(FleetError::Refused("Board owner changed".into()));
        }
        Ok(())
    };
    let check_live = || {
        let current =
            owner::resolve(global, route, id, true).map_err(|e| FleetError::Refused(e.message))?;
        if !current.owner.writable {
            return Err(FleetError::Refused("Board owner is read-only".into()));
        }
        Ok(())
    };
    // In-process MCP is still the authenticated CP operator, never an agent.
    let reply = Bus::open(&ctx.root.space, rupu_fleet::address::OPERATOR).operator_work_apply(
        command,
        OperatorVia::Cp,
        check_owner,
        check_live,
    )?;
    let current_item = source::Resolver::new(&ctx).item(&reply.current_item);
    bounded(0, |_| {
        Ok(
            json!({"version":1,"owner":ctx.owner,"receipt":reply.receipt,"replayed":reply.replayed,
        "current_item":current_item,"queue_revision":reply.queue_revision,
        "allowed_actions":actions(ctx.owner.writable,reply.current_item.state)}),
        )
    })
    .map_err(|_| {
        Error::reconciliation(
            &request_id,
            "Work committed but the response could not be returned; replay the identical request",
        )
    })
}
