//! Strict read-only selectors; all path/ancestry authority is server sealed.
use super::*;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ChildrenQuery {
    host: Option<String>,
    ws_id: String,
    parent_cursor: String,
    cursor: Option<String>,
    limit: Option<usize>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct OpenQuery {
    host: Option<String>,
    ws_id: String,
    parent_cursor: String,
    snapshot: String,
    record_id: String,
    limit: Option<usize>,
}
fn parent_token(token: &str) -> ApiResult<()> {
    if token.is_empty() || token.len() > 32 * 1024 {
        return Err(ApiError::bad_request("invalid parent window cursor"));
    }
    Ok(())
}
fn permit(s: &AppState) -> ApiResult<tokio::sync::OwnedSemaphorePermit> {
    s.workflow_observations
        .follow_jobs
        .clone()
        .try_acquire_owned()
        .map_err(|_| {
            ApiError(
                StatusCode::TOO_MANY_REQUESTS,
                "transcript window reads busy".into(),
            )
        })
}
pub(super) async fn children(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<ChildrenQuery>,
) -> ApiResult<Json<descendants::Reply>> {
    owner(&id, &q.ws_id, q.host.as_deref())?;
    parent_token(&q.parent_cursor)?;
    let limit = q.limit.unwrap_or(25);
    if !(1..=50).contains(&limit) || q.cursor.as_ref().is_some_and(|c| c.len() > 2048) {
        return Err(ApiError::bad_request("invalid descendant page selector"));
    }
    let permit = permit(&s)?;
    transcript_route::admitted(permit, move |cancel| {
        descendants::catalog(
            &s,
            &id,
            &q.ws_id,
            descendants::Query {
                parent_cursor: q.parent_cursor,
                cursor: q.cursor,
                limit,
            },
            &cancel,
        )
    })
    .await
    .map(Json)
}
pub(super) async fn open(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<OpenQuery>,
) -> ApiResult<Json<transcript_follow::Reply>> {
    owner(&id, &q.ws_id, q.host.as_deref())?;
    parent_token(&q.parent_cursor)?;
    let limit = q.limit.unwrap_or(100);
    if !(1..=200).contains(&limit)
        || q.snapshot.is_empty()
        || q.snapshot.len() > 64
        || q.record_id.is_empty()
        || q.record_id.len() > 80
    {
        return Err(ApiError::bad_request("invalid descendant selection"));
    }
    let permit = permit(&s)?;
    transcript_route::admitted(permit, move |cancel| {
        let member = descendants::select(
            &s,
            &id,
            &q.ws_id,
            descendants::Selection {
                parent_cursor: q.parent_cursor,
                snapshot: q.snapshot,
                record_id: q.record_id,
            },
            &cancel,
        )?;
        transcript_follow::read(
            &s,
            &id,
            &q.ws_id,
            transcript_follow::Query {
                action: transcript_follow::Action::Latest,
                cursor: None,
                limit,
            },
            Some(member),
            cancel,
        )
    })
    .await
    .map(Json)
}
