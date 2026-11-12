use super::*;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct FrontierQuery {
    host: Option<String>,
    ws_id: String,
    known_frontier: Option<String>,
}
pub(super) async fn get(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<FrontierQuery>,
) -> ApiResult<Json<Value>> {
    owner(&id, &q.ws_id, q.host.as_deref())?;
    if q.known_frontier.as_ref().is_some_and(|v| {
        v.len() != 71
            || !v.starts_with("sha256:")
            || !v[7..]
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    }) {
        return Err(ApiError::bad_request("invalid workflow frontier"));
    }
    let permit = s
        .workflow_observations
        .metadata
        .clone()
        .try_acquire_owned()
        .map_err(|_| {
            ApiError(
                StatusCode::TOO_MANY_REQUESTS,
                "workflow frontier reads busy".into(),
            )
        })?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        read(&s, &id, &q.ws_id, q.known_frontier.as_deref()).map(Json)
    })
    .await
    .map_err(|_| ApiError::internal("workflow frontier task failed"))?
}
pub(super) fn read(s: &AppState, id: &str, ws: &str, known: Option<&str>) -> ApiResult<Value> {
    let mut opened = super::owner::open(s, id, ws)?;
    for name in [
        "workflow.yaml",
        "step_results.jsonl",
        "unit_checkpoints.jsonl",
        "attempts.jsonl",
        "events.jsonl",
    ] {
        opened.input.observe(name)?;
    }
    let execution = executing::Observation::read(&opened)?;
    opened.verify(id, ws)?;
    execution.bind(&mut opened.input);
    let stamp = opened
        .input
        .frontier(id, ws, opened.store_kind, &s.workflow_observations.epoch);
    let value = json!({"schema":LIVE_FEATURE,"run_id":id,"workspace_id":ws,"host_id":"local","store_kind":opened.store_kind,
        "frontier":stamp,"changed":known!=Some(stamp.as_str()),"observed_at":chrono::Utc::now(),"comparison":"metadata_read_time",
        "run":serde_json::from_slice::<Value>(&opened.owner_bytes).expect("validated owner"),"input_bytes":opened.input.bytes,
        "executing_steps":execution.value,"execution_input_bytes":execution.input_bytes});
    if serde_json::to_vec(&value).expect("frontier JSON").len() > MAX_PAGE {
        return Err(too_large("workflow frontier response limit"));
    }
    Ok(value)
}
