//! The operator's side of a message space over HTTP (spec F1b §3.5), for
//! workflow and standalone runs, sessions and agentiflows alike:
//!
//! - `GET /api/{runs,sessions,agentiflows}/:id/messages?after=&limit=&kind=`
//!   → `MessagesView` (`rupu_runtime::services::operator`), the same wire
//!   shape `rupu message list --json` and `rupu __message list` print.
//! - `POST …/messages` `{to?, body, urgent?}` → `202 {id, delivery}`; 409 when
//!   the run isn't running. A write, so `cp serve` only (501 otherwise).
//! - `GET …/messages/stream` → an SSE tail of the space's log. This host only:
//!   `?host=<remote>` is a 501 (a remote is read by polling `GET`).
//!
//! `?host=<id>` routes the GET and POST through that host's connector
//! ([`HostConnector::list_messages`] / [`HostConnector::send_operator_message`]).
//! An id is served only by the route its prefix names
//! ([`MessageRoute::of_id`]): a session id on `/api/runs` is a 404.
//!
//! [`HostConnector::list_messages`]: crate::host::connector::HostConnector::list_messages
//! [`HostConnector::send_operator_message`]: crate::host::connector::HostConnector::send_operator_message

use std::convert::Infallible;
use std::time::Duration;

use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{
        sse::{Event as SseEvent, KeepAlive, Sse},
        IntoResponse, Response,
    },
    routing::get,
    Json, Router,
};
use futures_util::{Stream, StreamExt as _};
use rupu_fleet::LogLine;
use rupu_runtime::services::operator::{
    parse_kinds, MessageError, MessageView, MessagesView, OperatorSendReply, ViewQuery,
};
use serde::{Deserialize, Serialize};

use crate::error::{ApiError, ApiResult};
use crate::host::connector::{HostConnectorError, OperatorMessageRequest};
use crate::host::messages::{self, MessageRoute};
use crate::state::AppState;
use crate::transcript_tail::NumberedTail;

pub fn routes() -> Router<AppState> {
    let mut router = Router::new();
    for route in MessageRoute::ALL {
        let path = format!("/api/{}/:id/messages", route.segment());
        router = router
            .route(
                &path,
                get(
                    move |State(s): State<AppState>,
                          Path(id): Path<String>,
                          Query(q): Query<ListQuery>| async move {
                        list(route, &s, &id, q).await.map(Json)
                    },
                )
                .post(
                    move |State(s): State<AppState>,
                          Path(id): Path<String>,
                          Query(q): Query<HostQuery>,
                          headers: HeaderMap,
                          Json(body): Json<PostBody>| async move {
                        post(route, &s, &id, q.host.as_deref(), &headers, body).await
                    },
                ),
            )
            .route(
                &format!("{path}/stream"),
                get(
                    move |State(s): State<AppState>,
                          Path(id): Path<String>,
                          Query(q): Query<HostQuery>,
                          headers: HeaderMap| async move {
                        stream(route, &s, &id, q.host.as_deref(), &headers).await
                    },
                ),
            );
    }
    router
}

#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub(crate) struct ListQuery {
    host: Option<String>,
    after: Option<String>,
    limit: Option<usize>,
    /// Comma-separated kinds (`direct,operator`).
    kind: Option<String>,
}

#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub(crate) struct HostQuery {
    host: Option<String>,
}

/// The body of `POST …/messages`.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub(crate) struct PostBody {
    /// The recipient; the root's default (an agentiflow's lead, everyone
    /// elsewhere) when absent.
    #[serde(default)]
    pub to: Option<String>,
    pub body: String,
    #[serde(default)]
    pub urgent: bool,
}

/// The remote host `host` names, if any (absent and `local` are this host).
fn remote(host: Option<&str>) -> Option<&str> {
    host.filter(|h| *h != "local")
}

/// `id` belongs to `route`, or it's a 404 whichever host is asked.
fn check_route(route: MessageRoute, id: &str) -> ApiResult<()> {
    if MessageRoute::of_id(id) == Some(route) {
        Ok(())
    } else {
        Err(message_error(route.not_found(id)))
    }
}

/// A messaging failure on this host, at its code's status.
fn message_error(e: MessageError) -> ApiError {
    let status =
        StatusCode::from_u16(e.code.http_status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    ApiError(status, e.message)
}

/// A messaging failure through a remote host's connector, at the status the
/// host's own CP would answer: its 404, 400 and 409 kept; a host that can't
/// serve messages (no transport, an older rupu) 501; anything else as any
/// remote read ([`crate::api::runs::host_read_error`]).
fn host_message_error(host: &str, e: HostConnectorError) -> ApiError {
    use crate::api::runs::remote_error_text;
    match e {
        HostConnectorError::NotFound(m) => ApiError::not_found(m),
        HostConnectorError::Invalid(m) => ApiError::bad_request(m),
        HostConnectorError::Remote(400, m) => ApiError::bad_request(remote_error_text(&m)),
        HostConnectorError::Remote(409, m) => ApiError::conflict(remote_error_text(&m)),
        other => crate::api::runs::host_read_error(host, other),
    }
}

async fn list(
    route: MessageRoute,
    s: &AppState,
    id: &str,
    q: ListQuery,
) -> ApiResult<MessagesView> {
    check_route(route, id)?;
    let kinds = q
        .kind
        .as_deref()
        .map(parse_kinds)
        .transpose()
        .map_err(ApiError::bad_request)?;
    let view = ViewQuery {
        after: q.after,
        limit: q.limit.unwrap_or(usize::MAX),
        kinds,
    };
    if let Some(host) = remote(q.host.as_deref()) {
        let conn = crate::api::runs::resolve_host(s, host)?;
        return conn
            .list_messages(id, view)
            .await
            .map_err(|e| host_message_error(host, e));
    }
    let global = s.global_dir.clone();
    let id = id.to_string();
    tokio::task::spawn_blocking(move || messages::view(&global, &id, &view))
        .await
        .map_err(|e| ApiError::internal(format!("reading messages panicked: {e}")))?
        .map_err(message_error)
}

/// `POST …/messages`, and what `POST /api/agentiflows/:id/steer` without
/// `stop` sends ([`crate::api::agentiflows`]).
pub(crate) async fn post(
    route: MessageRoute,
    s: &AppState,
    id: &str,
    host: Option<&str>,
    headers: &HeaderMap,
    body: PostBody,
) -> ApiResult<Response> {
    check_route(route, id)?;
    crate::api::config::require_writable_to(s, "messaging a run")?;
    // The CP authenticates a shared token, not a person: there is no viewer
    // to record.
    // Only the in-process MCP operator source sends this header, and only a
    // local write records it: a remote host keeps its own attribution.
    let client = (remote(host).is_none()
        && headers.get("x-rupu-client").and_then(|v| v.to_str().ok()) == Some("mcp"))
    .then(|| "mcp".to_string());
    let msg = OperatorMessageRequest {
        to: body.to,
        body: body.body,
        urgent: body.urgent,
        viewer: None,
        client,
    };
    let reply: OperatorSendReply = if let Some(host) = remote(host) {
        let conn = crate::api::runs::resolve_host(s, host)?;
        conn.send_operator_message(id, msg)
            .await
            .map_err(|e| host_message_error(host, e))?
    } else {
        let caps = s
            .config
            .read()
            .map(|c| rupu_runtime::services::messaging::caps(&c))
            .unwrap_or_default();
        let global = s.global_dir.clone();
        let id = id.to_string();
        tokio::task::spawn_blocking(move || messages::send(&global, &id, msg, caps))
            .await
            .map_err(|e| ApiError::internal(format!("sending the message panicked: {e}")))?
            .map_err(message_error)?
    };
    Ok((StatusCode::ACCEPTED, Json(reply)).into_response())
}

/// One frame of `GET …/messages/stream`: a message as the view shows it
/// (with no receipts yet), or a receipt for one sent earlier.
#[derive(Serialize)]
#[serde(tag = "t", rename_all = "snake_case")]
enum StreamItem {
    Msg(Box<MessageView>),
    Seen {
        id: String,
        by: String,
        turn: u32,
        at: String,
    },
}

impl From<LogLine> for StreamItem {
    fn from(line: LogLine) -> Self {
        match line {
            LogLine::Msg(message) => StreamItem::Msg(Box::new(MessageView {
                message,
                seen: Vec::new(),
            })),
            LogLine::Seen { id, by, turn, at } => StreamItem::Seen { id, by, turn, at },
        }
    }
}

/// The space's log as SSE, every line from the start (or after a
/// reconnect's `Last-Event-ID`, a line number), then each line as it is
/// appended. It stays open: a finished run's space can still be read, and
/// a client closes it when it is done.
async fn stream(
    route: MessageRoute,
    s: &AppState,
    id: &str,
    host: Option<&str>,
    headers: &HeaderMap,
) -> ApiResult<Sse<impl Stream<Item = Result<SseEvent, Infallible>>>> {
    check_route(route, id)?;
    if let Some(host) = remote(host) {
        return Err(ApiError::not_available(format!(
            "host {host}: a message stream is served for this host's runs only; remote hosts: \
             poll GET /api/{}/{id}/messages?host={host}",
            route.segment()
        )));
    }
    let global = s.global_dir.clone();
    let owned = id.to_string();
    let log = tokio::task::spawn_blocking(move || {
        messages::root(&global, &owned).map(|(_, root)| root.log_path())
    })
    .await
    .map_err(|e| ApiError::internal(format!("resolving the message space panicked: {e}")))?
    .map_err(message_error)?;
    let after = crate::sse::last_event_id(headers);
    let frames = NumberedTail::<LogLine>::open(&log, after).filter_map(|line| async move {
        let event = line.event.filter(|_| line.send)?;
        let frame = SseEvent::default()
            .id(line.id.to_string())
            .json_data(StreamItem::from(event))
            .unwrap_or_else(|_| SseEvent::default().comment("message serialize error"));
        Some(Ok(frame))
    });
    Ok(Sse::new(frames).keep_alive(KeepAlive::new().interval(Duration::from_secs(15))))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_host_failure_keeps_the_status_its_own_cp_would_answer() {
        let status = |e| host_message_error("h", e).0.as_u16();
        assert_eq!(status(HostConnectorError::NotFound("x".into())), 404);
        assert_eq!(status(HostConnectorError::Invalid("x".into())), 400);
        assert_eq!(status(HostConnectorError::Remote(400, "x".into())), 400);
        assert_eq!(
            status(HostConnectorError::Remote(
                409,
                r#"{"error":"not running"}"#.into()
            )),
            409
        );
        assert_eq!(status(HostConnectorError::Unsupported("x".into())), 501);
        assert_eq!(status(HostConnectorError::Remote(501, "x".into())), 501);
        assert_eq!(status(HostConnectorError::NotJson("x".into())), 501);
        assert_eq!(status(HostConnectorError::Unreachable("x".into())), 502);
        assert_eq!(status(HostConnectorError::Remote(500, "x".into())), 502);
        assert_eq!(
            host_message_error(
                "h",
                HostConnectorError::Remote(409, r#"{"error":"not running"}"#.into())
            )
            .1,
            "not running"
        );
    }
}
