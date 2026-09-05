//! `GET /api/transcript` — read a JSONL transcript file with a
//! security-critical path validator.
//!
//! The validator ([`validate_transcript_path`]) is the security boundary: it
//! prevents the `?path=` query parameter from being used to read arbitrary
//! files off the host. It canonicalizes the requested path (resolving `..`
//! traversal and symlinks) and then requires the resolved path to live inside
//! one of the canonicalized allowed roots.

use crate::{
    error::{ApiError, ApiResult},
    host::connector::{FeedGuard, HostConnector, HostConnectorError},
    state::AppState,
};
use axum::{
    extract::{Query, State},
    response::{
        sse::{Event as SseEvent, KeepAlive, Sse},
        IntoResponse, Response,
    },
    routing::get,
    Json, Router,
};
use futures_util::{Stream, StreamExt as _};
use std::convert::Infallible;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// Validate a requested transcript `?path=`: require a `.jsonl` file, reject
/// `..` traversal, and require the path to resolve inside one of `allowed_roots`
/// (themselves canonicalized). Symlink-escape is defeated by canonicalizing the
/// deepest existing ancestor before the `starts_with` check.
///
/// Unlike a plain `canonicalize`, this does NOT require the target file to
/// exist: a freshly-sent turn's transcript is validated before its worker has
/// written the `.jsonl`, so the UI can open it and stream it in (the old
/// canonicalize-the-whole-path approach returned a misleading "cannot resolve
/// path" for that race).
pub fn validate_transcript_path(raw: &str, allowed_roots: &[PathBuf]) -> Result<PathBuf, String> {
    let p = Path::new(raw);
    if p.extension().and_then(|e| e.to_str()) != Some("jsonl") {
        return Err("not a .jsonl file".into());
    }
    // Reject `..` outright — the web only ever passes already-resolved absolute
    // transcript paths, and forbidding ParentDir removes traversal as a vector
    // before we touch the filesystem.
    if p.components().any(|c| c == std::path::Component::ParentDir) {
        return Err("path must not contain ..".into());
    }
    // The transcript file may not exist yet (a freshly-sent turn whose worker
    // hasn't written the `.jsonl` — that race previously produced a misleading
    // "cannot resolve path"). Resolve symlinks on the deepest EXISTING ancestor
    // for the security check, then re-append the not-yet-created remainder. An
    // existing file is still fully canonicalized (no loss of symlink-escape
    // protection for real files).
    let resolved = if p.exists() {
        std::fs::canonicalize(p).map_err(|_| "cannot resolve path".to_string())?
    } else {
        canonicalize_existing_prefix(p).ok_or_else(|| "cannot resolve path".to_string())?
    };
    for root in allowed_roots {
        if let Ok(rc) = std::fs::canonicalize(root) {
            if resolved.starts_with(&rc) {
                return Ok(resolved);
            }
        }
    }
    Err("path is outside the allowed roots".into())
}

/// Canonicalize the deepest existing ancestor of `p` (resolving symlinks in the
/// real prefix) and re-append the remaining, not-yet-created components. Returns
/// `None` only if no ancestor exists. `p` must be `..`-free (the caller rejects
/// ParentDir first).
fn canonicalize_existing_prefix(p: &Path) -> Option<PathBuf> {
    let mut ancestor = p;
    let mut tail: Vec<&std::ffi::OsStr> = Vec::new();
    loop {
        if ancestor.exists() {
            let mut out = std::fs::canonicalize(ancestor).ok()?;
            for seg in tail.iter().rev() {
                out.push(seg);
            }
            return Some(out);
        }
        match (ancestor.file_name(), ancestor.parent()) {
            (Some(name), Some(parent)) => {
                tail.push(name);
                ancestor = parent;
            }
            _ => return None,
        }
    }
}

#[derive(serde::Deserialize)]
struct PathQ {
    path: String,
    /// When present and not `"local"`, serve from / through the named host.
    #[serde(default)]
    host: Option<String>,
    /// Spec §3.3: the run that recorded `path`. Required only when `host` is
    /// remote and the transcript has not been mirrored or cached yet — the
    /// connector uses it to check the path is one the run itself claims.
    #[serde(default)]
    run: Option<String>,
}

/// The set of directories a transcript path is allowed to resolve into: the
/// CP global dir plus every registered workspace's path (project-local
/// transcripts/coverage live under `<project>/.rupu/`).
fn allowed_roots(s: &AppState) -> Vec<PathBuf> {
    let mut roots = vec![s.global_dir.clone()];
    if let Ok(list) = (rupu_workspace::WorkspaceStore {
        root: s.global_dir.join("workspaces"),
    })
    .list()
    {
        roots.extend(list.into_iter().map(|w| PathBuf::from(w.path)));
    }
    roots
}

/// Where a `?host=<remote>` transcript request is served from (§3.3 steps 1–3).
enum RemoteRead {
    /// A validated, existing local file (the PR #646 agent-run mirror).
    Local(PathBuf),
    /// The connector's cache file; `complete` = has a `.complete` sidecar.
    Cache { cache: PathBuf, complete: bool },
}

/// Pure decision: `local` is the already-validated local path if it exists,
/// `mapped` is `connector.local_transcript_path(raw)`.
fn plan_remote_read(local: Option<PathBuf>, mapped: PathBuf) -> RemoteRead {
    if let Some(p) = local {
        return RemoteRead::Local(p);
    }
    let complete = crate::host::transcript_paths::is_complete(&mapped);
    RemoteRead::Cache {
        cache: mapped,
        complete,
    }
}

/// Resolve the connector and the read plan for a remote-host request.
fn resolve_remote(
    s: &AppState,
    host_id: &str,
    raw: &str,
) -> ApiResult<(Arc<dyn HostConnector>, RemoteRead)> {
    let conn = s.hosts.resolve(host_id).map_err(|e| match e {
        HostConnectorError::NotFound(_) => ApiError::not_found(format!("host {host_id} not found")),
        other => ApiError::internal(other.to_string()),
    })?;
    let local = validate_transcript_path(raw, &allowed_roots(s))
        .ok()
        .filter(|p| p.exists());
    let mapped = conn.local_transcript_path(Path::new(raw));
    let plan = plan_remote_read(local, mapped);
    Ok((conn, plan))
}

/// Read a local transcript file into the `{events, summary, unparsed}` page.
/// A missing file is an empty page (a freshly-sent turn's transcript).
fn read_local_page(path: &Path) -> ApiResult<serde_json::Value> {
    if !path.exists() {
        return Ok(serde_json::json!({ "events": [], "summary": null }));
    }
    let (events, unparsed) =
        read_events_counting_unparsed(path).map_err(|e| ApiError::internal(e.to_string()))?;
    let summary = rupu_transcript::JsonlReader::summary(path).ok();
    Ok(serde_json::json!({ "events": events, "summary": summary, "unparsed": unparsed }))
}

/// §4.2: the coordinator could not collect the rest of this transcript.
fn mark_partial(mut page: serde_json::Value) -> serde_json::Value {
    page["partial"] = serde_json::json!(true);
    page
}

fn map_conn_err(host_id: &str, e: HostConnectorError) -> ApiError {
    match e {
        HostConnectorError::NotFound(m) => ApiError::not_found(m),
        HostConnectorError::Invalid(m) | HostConnectorError::Unsupported(m) => {
            ApiError::bad_request(m)
        }
        HostConnectorError::Unreachable(m) => {
            ApiError::bad_gateway(format!("host {host_id} unreachable: {m}"))
        }
        other => ApiError::internal(other.to_string()),
    }
}

/// Validate `raw` against the LOCAL allowed roots. This is the legacy
/// fallback path for a connector that doesn't implement the lazy cache
/// (`Unsupported` from `pull_transcript` / `ensure_transcript_feed`): today's
/// behaviour for HTTP hosts is to proxy at the connector level (its own
/// `get_transcript` call), and for tunnel/bucket hosts a remote-only recorded
/// path is never found under the local roots, so this simply 400s "outside
/// the allowed roots" — exactly what happens on `main` for those host types.
fn validate_local(s: &AppState, raw: &str) -> ApiResult<PathBuf> {
    validate_transcript_path(raw, &allowed_roots(s)).map_err(ApiError::bad_request)
}

/// Classifies the outcome of `HostConnector::pull_transcript` for
/// `get_transcript`'s remote cache-miss arm.
enum PullOutcome {
    /// The pull succeeded; serve the (now up to date) cache file.
    ServeCache,
    /// The host was unreachable, but a previous pull already left something
    /// on disk — serve it, flagged (§4.2).
    ServePartial,
    /// This connector doesn't implement the lazy cache at all (the trait
    /// default `Unsupported`) — fall back to its own whole-page
    /// `get_transcript`, i.e. today's behaviour for HTTP/Tunnel/Bucket hosts.
    ProxyLegacy,
    /// Any other error, already mapped to its final `ApiError`.
    Fail(ApiError),
}

/// Pure classification of a `pull_transcript` result. `cache` is inspected
/// only in the `Unreachable` case (§4.2's partial-serve rule).
fn map_pull_result(
    host_id: &str,
    cache: &Path,
    res: Result<(), HostConnectorError>,
) -> PullOutcome {
    match res {
        Ok(()) => PullOutcome::ServeCache,
        Err(HostConnectorError::Unreachable(_)) if cache.exists() => PullOutcome::ServePartial,
        Err(HostConnectorError::Unsupported(_)) => PullOutcome::ProxyLegacy,
        Err(e) => PullOutcome::Fail(map_conn_err(host_id, e)),
    }
}

/// Attach `guard` to `stream`'s lifetime: it is held until the returned
/// stream itself is dropped, then released. Used to keep a remote transcript
/// feed's [`FeedGuard`] alive for exactly as long as the SSE stream tailing
/// its cache file — no longer, no shorter, and independent of anything
/// axum/SSE-specific (kept as a pure `Stream` combinator so the mechanic can
/// be pinned by a unit test without spinning up an HTTP server).
fn hold_while_streaming<S>(stream: S, guard: Option<FeedGuard>) -> impl Stream<Item = S::Item>
where
    S: Stream,
{
    stream.map(move |item| {
        let _keep = &guard;
        item
    })
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/transcript", get(get_transcript))
        .route("/api/transcript/stream", get(stream_transcript))
}

/// `GET /api/transcript?path=<path>[&host=<id>&run=<id>]`
///
/// Without `?host=` (or `?host=local`): validate the path against local
/// allowed roots and read from disk (unchanged behaviour).
///
/// With `?host=<remote-id>`: read through the connector's lazy transcript
/// cache (spec §3.3). A validated local file wins outright; otherwise a
/// complete cache is served as-is; an incomplete/absent cache WITHOUT `run`
/// falls back to the connector's own `get_transcript` (no run-scoped claim to
/// check yet); WITH `run` it triggers `pull_transcript` (`run` checks the
/// path is one the run itself claims) — and if THAT comes back `Unsupported`
/// (the connector never implemented the lazy cache: HTTP, tunnel, bucket),
/// falls back to `get_transcript` all the same, so a client that always sends
/// `run` doesn't regress those host types (see [`PullOutcome::ProxyLegacy`]).
/// Unknown host id → 404; a host unreachable with nothing cached on disk →
/// 502; unreachable with a partial file on disk → 200 with `"partial": true`.
async fn get_transcript(
    State(s): State<AppState>,
    Query(q): Query<PathQ>,
) -> ApiResult<Json<serde_json::Value>> {
    let host_id = q.host.as_deref().unwrap_or("local");

    if host_id != "local" {
        let (conn, plan) = resolve_remote(&s, host_id, &q.path)?;
        return match plan {
            RemoteRead::Local(p) => Ok(Json(read_local_page(&p)?)),
            RemoteRead::Cache {
                cache,
                complete: true,
            } => Ok(Json(read_local_page(&cache)?)),
            RemoteRead::Cache {
                cache,
                complete: false,
            } => {
                // No `run`: this connector's transcripts aren't behind the
                // lazy cache at all — HTTP/Tunnel/Bucket hosts still serve
                // `GET /api/transcript` by proxying `get_transcript` end to
                // end (design §10 non-goal: "they keep today's behaviour").
                // Only a caller that knows it is reading a run-scoped,
                // not-yet-mirrored path (the SSH case) supplies `run`.
                let Some(run_id) = q.run.as_deref() else {
                    let result = conn
                        .get_transcript(&q.path)
                        .await
                        .map_err(|e| map_conn_err(host_id, e))?;
                    return Ok(Json(result));
                };
                let terminal = s
                    .run_store
                    .load(run_id)
                    .map(|r| r.status.is_terminal())
                    .unwrap_or(false);
                let pull = conn
                    .pull_transcript(run_id, Path::new(&q.path), terminal)
                    .await;
                match map_pull_result(host_id, &cache, pull) {
                    PullOutcome::ServeCache => Ok(Json(read_local_page(&cache)?)),
                    PullOutcome::ServePartial => Ok(Json(mark_partial(read_local_page(&cache)?))),
                    // This connector never implemented the lazy cache (HTTP,
                    // tunnel, bucket) — fall back to its own whole-page proxy,
                    // exactly as it behaved before this endpoint learned `run`.
                    PullOutcome::ProxyLegacy => {
                        let result = conn
                            .get_transcript(&q.path)
                            .await
                            .map_err(|e| map_conn_err(host_id, e))?;
                        Ok(Json(result))
                    }
                    PullOutcome::Fail(e) => Err(e),
                }
            }
        };
    }

    // Local path: validate against allowed roots then read from disk.
    let path = validate_local(&s, &q.path)?;
    Ok(Json(read_local_page(&path)?))
}

/// Read all parseable events, counting unparseable lines — EXCEPT a parse
/// failure on the final line, which is the signature of a mid-write tail on
/// a live transcript, not corruption.
fn read_events_counting_unparsed(
    path: &Path,
) -> Result<(Vec<rupu_transcript::Event>, usize), rupu_transcript::ReadError> {
    let mut events = Vec::new();
    let mut unparsed = 0usize;
    let mut last_line_failed = false;
    for ev in rupu_transcript::JsonlReader::iter(path)? {
        match ev {
            Ok(e) => {
                events.push(e);
                last_line_failed = false;
            }
            Err(_) => {
                unparsed += 1;
                last_line_failed = true;
            }
        }
    }
    if last_line_failed {
        unparsed -= 1;
    }
    Ok((events, unparsed))
}

/// `GET /api/transcript/stream?path=[&host=<id>&run=<id>]` — SSE live-tail of
/// a transcript JSONL.
///
/// Validation runs first and is the SAME security boundary as the static
/// [`get_transcript`] endpoint (400 on an invalid / out-of-root path). On
/// success, opens a [`TranscriptTail`] and maps each parsed
/// [`rupu_transcript::Event`] to an SSE `data:` line of JSON; the connection
/// stays open, emitting events as the transcript grows.
///
/// With `?host=<remote-id>`: the same lazy-cache plan as [`get_transcript`],
/// except an incomplete cache is fed by [`HostConnector::ensure_transcript_feed`]
/// rather than a one-shot pull — the returned [`FeedGuard`] is held for the
/// whole lifetime of the SSE stream so the remote tail is released the moment
/// the client disconnects. A connector that returns `Unsupported` (HTTP,
/// tunnel, bucket — no lazy cache) falls back to validating `path` against
/// the LOCAL allowed roots and tailing it locally, i.e. today's behaviour
/// (typically a 400, since a genuinely remote path is never local).
///
/// [`TranscriptTail`]: crate::transcript_tail::TranscriptTail
async fn stream_transcript(State(s): State<AppState>, Query(q): Query<PathQ>) -> Response {
    let host_id = q.host.as_deref().unwrap_or("local");
    // Held for the SSE stream's lifetime (moved into the map closure below).
    let mut guard: Option<FeedGuard> = None;
    let path = if host_id == "local" {
        match validate_local(&s, &q.path) {
            Ok(p) => p,
            Err(e) => return e.into_response(),
        }
    } else {
        match resolve_remote(&s, host_id, &q.path) {
            Err(e) => return e.into_response(),
            Ok((_, RemoteRead::Local(p))) => p,
            Ok((
                _,
                RemoteRead::Cache {
                    cache,
                    complete: true,
                },
            )) => cache,
            Ok((
                conn,
                RemoteRead::Cache {
                    cache,
                    complete: false,
                },
            )) => {
                let Some(run_id) = q.run.as_deref() else {
                    return ApiError::bad_request(
                        "run is required to stream a transcript that has not been mirrored yet",
                    )
                    .into_response();
                };
                match conn
                    .ensure_transcript_feed(run_id, Path::new(&q.path))
                    .await
                {
                    Ok(g) => {
                        guard = Some(g);
                        cache
                    }
                    // This connector never implemented the lazy cache (HTTP,
                    // tunnel, bucket) — fall back to today's local-only
                    // validate+tail (a remote-only path 400s "outside the
                    // allowed roots", exactly as it does on `main`).
                    Err(HostConnectorError::Unsupported(_)) => match validate_local(&s, &q.path) {
                        Ok(p) => p,
                        Err(e) => return e.into_response(),
                    },
                    Err(e) => return map_conn_err(host_id, e).into_response(),
                }
            }
        }
    };
    let tail = match crate::transcript_tail::TranscriptTail::open(&path).await {
        Ok(t) => t,
        Err(e) => return ApiError::internal(e.to_string()).into_response(),
    };
    let stream = hold_while_streaming(tail, guard).map(|ev| {
        let sse = SseEvent::default()
            .json_data(&ev)
            .unwrap_or_else(|_| SseEvent::default().comment("event serialize error"));
        Ok::<_, Infallible>(sse)
    });
    Sse::new(stream)
        .keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
        .into_response()
}

#[cfg(test)]
mod remote_read_tests {
    use super::*;

    #[test]
    fn plan_prefers_a_local_file_then_a_complete_cache_then_a_pull() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = tmp.path().join("mirror/h/transcripts/run_01A.jsonl");

        // 1. A validated, existing local file wins outright.
        let local = tmp.path().join("transcripts/run_01A.jsonl");
        std::fs::create_dir_all(local.parent().unwrap()).unwrap();
        std::fs::write(&local, "").unwrap();
        assert!(matches!(
            plan_remote_read(Some(local.clone()), cache.clone()),
            RemoteRead::Local(p) if p == local
        ));

        // 2. No local file: an incomplete (or absent) cache means "pull".
        assert!(matches!(
            plan_remote_read(None, cache.clone()),
            RemoteRead::Cache {
                complete: false,
                ..
            }
        ));

        // 3. A complete cache is served as-is.
        std::fs::create_dir_all(cache.parent().unwrap()).unwrap();
        std::fs::write(&cache, "").unwrap();
        std::fs::write(crate::host::transcript_paths::complete_marker(&cache), b"").unwrap();
        assert!(matches!(
            plan_remote_read(None, cache.clone()),
            RemoteRead::Cache { complete: true, .. }
        ));
    }

    #[test]
    fn plan_treats_an_identity_mapping_as_a_pull_target_too() {
        // A connector that maps to itself (Task 2 default) still lands in
        // the run-scoped branch, where the connector's own allowlist decides.
        let raw = Path::new("/remote/x/run_01A.jsonl");
        assert!(matches!(
            plan_remote_read(None, raw.to_path_buf()),
            RemoteRead::Cache { complete: false, cache } if cache == raw
        ));
    }

    #[test]
    fn partial_flag_is_absent_unless_set() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("t.jsonl");
        std::fs::write(&p, "{\"type\":\"turn_start\",\"data\":{\"turn_idx\":0}}\n").unwrap();
        let page = read_local_page(&p).unwrap();
        assert!(page.get("partial").is_none());
        let flagged = mark_partial(page);
        assert_eq!(flagged["partial"], serde_json::json!(true));
    }

    #[test]
    fn map_pull_result_classifies_every_outcome() {
        use axum::http::StatusCode;

        let tmp = tempfile::tempdir().unwrap();

        // Ok → serve the (now current) cache, whether or not it existed before.
        let cache = tmp.path().join("t.jsonl");
        assert!(matches!(
            map_pull_result("h", &cache, Ok(())),
            PullOutcome::ServeCache
        ));

        // Unreachable + something already on disk → serve it, partial.
        std::fs::write(&cache, "").unwrap();
        assert!(matches!(
            map_pull_result(
                "h",
                &cache,
                Err(HostConnectorError::Unreachable("down".into()))
            ),
            PullOutcome::ServePartial
        ));

        // Unreachable + nothing on disk → a hard failure, 502.
        let absent = tmp.path().join("missing.jsonl");
        match map_pull_result(
            "h",
            &absent,
            Err(HostConnectorError::Unreachable("down".into())),
        ) {
            PullOutcome::Fail(e) => assert_eq!(e.0, StatusCode::BAD_GATEWAY),
            _ => panic!("expected Fail(502) for an unreachable host with nothing cached"),
        }

        // Unsupported → this connector never implemented the lazy cache;
        // the caller falls back to the connector's own `get_transcript`.
        assert!(matches!(
            map_pull_result(
                "h",
                &cache,
                Err(HostConnectorError::Unsupported("no".into()))
            ),
            PullOutcome::ProxyLegacy
        ));

        // Invalid (the run doesn't claim this path, or it's owned by another
        // host) → a hard failure, 400.
        match map_pull_result("h", &cache, Err(HostConnectorError::Invalid("bad".into()))) {
            PullOutcome::Fail(e) => assert_eq!(e.0, StatusCode::BAD_REQUEST),
            _ => panic!("expected Fail(400) for an invalid pull"),
        }
    }

    /// Pins the `FeedGuard` lifetime mechanic `stream_transcript` relies on:
    /// the guard must survive every item the stream yields and be dropped
    /// exactly when the stream itself is dropped (client disconnect) — not
    /// before (mid-stream) and not held forever (leaking the remote feed).
    #[tokio::test]
    async fn hold_while_streaming_keeps_the_guard_alive_until_the_stream_drops() {
        use std::sync::atomic::{AtomicBool, Ordering};

        struct DropFlag(Arc<AtomicBool>);
        impl Drop for DropFlag {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }

        let dropped = Arc::new(AtomicBool::new(false));
        let guard = FeedGuard::holding(Box::new(DropFlag(dropped.clone())));

        // `Box::pin`, not `std::pin::pin!`: the latter only shadows a local
        // with a `Pin<&mut _>`, so dropping the binding wouldn't drop the
        // stream (and its held guard) at all — the boxed form actually owns
        // the stream, so `drop(stream)` below is a real drop.
        let mut stream = Box::pin(hold_while_streaming(
            futures_util::stream::iter([1, 2]),
            Some(guard),
        ));

        assert_eq!(stream.next().await, Some(1));
        assert!(
            !dropped.load(Ordering::SeqCst),
            "guard must still be alive after the first item"
        );

        assert_eq!(stream.next().await, Some(2));
        assert!(
            !dropped.load(Ordering::SeqCst),
            "guard must still be alive after the last item, before the stream itself drops"
        );

        drop(stream);
        assert!(
            dropped.load(Ordering::SeqCst),
            "dropping the stream must release the guard"
        );
    }
}

#[cfg(test)]
mod read_tests {
    use super::read_events_counting_unparsed;
    use std::io::Write as _;

    #[test]
    fn unparsed_counts_bad_lines_but_forgives_a_truncated_tail() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("t.jsonl");
        let mut f = std::fs::File::create(&p).unwrap();
        writeln!(f, r#"{{"type":"turn_start","data":{{"turn_idx":0}}}}"#).unwrap();
        writeln!(f, "THIS IS NOT JSON").unwrap();
        writeln!(f, r#"{{"type":"turn_end","data":{{"turn_idx":0}}}}"#).unwrap();
        write!(f, r#"{{"type":"assistant_delta","data":{{"conte"#).unwrap(); // torn tail
        let (events, unparsed) = read_events_counting_unparsed(&p).unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(
            unparsed, 1,
            "mid-file garbage counts; the torn final line does not"
        );
    }
}
