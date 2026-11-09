use super::transcript_follow::{Action, Query as ReadQuery, Reply};
use super::*;
use tokio_util::sync::CancellationToken;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct TranscriptQuery {
    host: Option<String>,
    ws_id: String,
    action: Action,
    cursor: Option<String>,
    limit: Option<usize>,
    snapshot: Option<String>,
    section: Option<String>,
    parent: Option<String>,
    step_id: Option<String>,
    position: Option<usize>,
    record_id: Option<String>,
}
pub(super) async fn get(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<TranscriptQuery>,
) -> ApiResult<Json<Reply>> {
    owner(&id, &q.ws_id, q.host.as_deref())?;
    let limit = q.limit.unwrap_or(100);
    if !(1..=200).contains(&limit) || q.cursor.as_ref().is_some_and(|c| c.len() > 32 * 1024) {
        return Err(ApiError::bad_request(
            "invalid transcript window limit or cursor",
        ));
    }
    let selected = if q.cursor.is_some() {
        if q.snapshot.is_some()
            || q.section.is_some()
            || q.parent.is_some()
            || q.step_id.is_some()
            || q.position.is_some()
            || q.record_id.is_some()
        {
            return Err(ApiError::bad_request(
                "cursor and initial member selectors are mutually exclusive",
            ));
        }
        None
    } else {
        if !matches!(q.action, Action::Latest) {
            return Err(ApiError::bad_request("initial member read requires latest"));
        }
        let required = || ApiError::bad_request("complete exact captured member selector required");
        Some(member::Selector {
            snapshot: q.snapshot.ok_or_else(required)?,
            section: q.section.ok_or_else(required)?,
            parent: q.parent,
            step_id: q.step_id,
            position: q.position.ok_or_else(required)?,
            record_id: q.record_id.ok_or_else(required)?,
        })
    };
    let permit = s
        .workflow_observations
        .follow_jobs
        .clone()
        .try_acquire_owned()
        .map_err(|_| {
            ApiError(
                StatusCode::TOO_MANY_REQUESTS,
                "transcript window reads busy".into(),
            )
        })?;
    admitted(permit, move |cancel| {
        if cancel.is_cancelled() {
            return Err(ApiError(
                StatusCode::SERVICE_UNAVAILABLE,
                "transcript_window_cancelled".into(),
            ));
        }
        let initial = selected
            .as_ref()
            .map(|selector| member::initial_with_cancel(&s, &id, &q.ws_id, selector, &cancel))
            .transpose()?;
        transcript_follow::read(
            &s,
            &id,
            &q.ws_id,
            ReadQuery {
                action: q.action,
                cursor: q.cursor,
                limit,
            },
            initial,
            cancel,
        )
    })
    .await
    .map(Json)
}
/// Cancellation follows the HTTP future, while the permit follows actual work.
/// A blocked kernel read cannot be forcibly cancelled; at most four workers
/// retain permits until their bounded cooperative work exits.
pub(super) async fn admitted<T: Send + 'static>(
    permit: tokio::sync::OwnedSemaphorePermit,
    work: impl FnOnce(CancellationToken) -> ApiResult<T> + Send + 'static,
) -> ApiResult<T> {
    let cancel = CancellationToken::new();
    let _guard = cancel.clone().drop_guard();
    let worker_cancel = cancel.clone();
    let worker = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        work(worker_cancel)
    });
    match tokio::time::timeout(Duration::from_secs(5), worker).await {
        Ok(Ok(result)) => result,
        Ok(Err(_)) => Err(ApiError::internal("transcript window task failed")),
        Err(_) => {
            cancel.cancel();
            Err(ApiError(
                StatusCode::SERVICE_UNAVAILABLE,
                "transcript_window_deadline".into(),
            ))
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn dropped_http_waiter_cancels_worker_but_permit_stays_with_work() {
        let gate = Arc::new(Semaphore::new(1));
        let permit = gate.clone().acquire_owned().await.unwrap();
        let (started, ready) = tokio::sync::oneshot::channel();
        let (finished, done) = tokio::sync::oneshot::channel();
        let waiter = tokio::spawn(admitted(permit, move |cancel| {
            let _ = started.send(());
            let deadline = Instant::now() + Duration::from_secs(2);
            while !cancel.is_cancelled() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(1));
            }
            let observed = cancel.is_cancelled();
            let _ = finished.send(observed);
            Ok(())
        }));
        ready.await.unwrap();
        assert_eq!(gate.available_permits(), 0);
        waiter.abort();
        assert!(done.await.unwrap());
        let released = tokio::time::timeout(Duration::from_secs(2), gate.clone().acquire_owned())
            .await
            .unwrap()
            .unwrap();
        drop(released);
        assert_eq!(gate.available_permits(), 1);
    }
}
