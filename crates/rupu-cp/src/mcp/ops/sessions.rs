//! The sessions operator tools, and the message log every run, session and
//! agentiflow has.

use super::{op, OpSpec};
use crate::api::{agents, messages, sessions};
use http::Method;

pub(super) fn ops() -> Vec<OpSpec> {
    vec![
        op(
            "sessions.list",
            Method::GET,
            "/api/sessions",
            "List interactive agent sessions, newest first.\n\nFilter with `q` (the query line, e.g. `is:active`), `scope`, `since`/`until`, `customer` and `host`, and page with `offset`/`limit`. Returns a JSON array of session rows; with `facets` set, an object `{rows, facets, facets_partial}`.",
        )
        .query::<sessions::SessionsQuery>(),
        op(
            "sessions.get",
            Method::GET,
            "/api/sessions/:id",
            "Get one session's full record.\n\n`id` is the session id; `host` picks a remote host. Returns the session detail: status, agent, limits and totals.",
        )
        .query::<sessions::SessionHostQuery>(),
        op(
            "sessions.turns",
            Method::GET,
            "/api/sessions/:id/runs",
            "List the turns (runs) a session has taken.\n\n`id` is the session id; `host` picks a remote host. Returns a JSON array of the session's runs, one per turn.",
        )
        .query::<sessions::SessionHostQuery>(),
        op(
            "sessions.usage_timeline",
            Method::GET,
            "/api/sessions/:id/usage-timeline",
            "Get a session's token and cost usage over time.\n\n`id` is the session id; `host` picks a remote host. Returns a JSON array of usage points.",
        )
        .query::<sessions::SessionHostQuery>(),
        op(
            "sessions.archive",
            Method::POST,
            "/api/sessions/:id/archive",
            "Archive a session so it leaves the active list.\n\n`id` is the session id; `host` picks a remote host. Returns `{ok, id}` (plus `host_id` for a remote host).",
        )
        .query::<sessions::SessionHostQuery>(),
        op(
            "sessions.restore",
            Method::POST,
            "/api/sessions/:id/restore",
            "Restore an archived session to the active list.\n\n`id` is the session id; `host` picks a remote host. Returns `{ok, id}` (plus `host_id` for a remote host).",
        )
        .query::<sessions::SessionHostQuery>(),
        op(
            "sessions.delete",
            Method::DELETE,
            "/api/sessions/:id",
            "Delete a session and its record permanently.\n\n`id` is the session id; `host` picks a remote host. Returns `{ok, id}` (plus `host_id` for a remote host).",
        )
        .query::<sessions::SessionHostQuery>(),
        op(
            "sessions.start",
            Method::POST,
            "/api/agents/:name/session",
            "Start an interactive agent session.\n\n`name` is the agent; `prompt` is the first message; `mode`, `target`, `working_dir`, `host` and `skills` are optional. Returns the new session id.",
        )
        .body::<agents::SessionStartBody>()
        .open_world(),
        op(
            "sessions.send",
            Method::POST,
            "/api/sessions/:id/send",
            "Send a message to a live session, starting a new turn.\n\n`id` is the session id; `prompt` is the message and `skills` are skills to invoke (at least one is required). Returns the run id of the turn.",
        )
        .query::<sessions::SendQuery>()
        .body::<sessions::SendBody>()
        .open_world(),
        op(
            "messages.list",
            Method::GET,
            "/api/{root}/:id/messages",
            "Read the message log of a run, session or agentiflow.\n\n`id` is the root's id (its prefix picks run, session or agentiflow); `after` is a message id to continue from, `limit` caps the page, `kind` filters (comma-separated). Returns the participants and messages.",
        )
        .query::<messages::ListQuery>(),
        op(
            "messages.send",
            Method::POST,
            "/api/{root}/:id/messages",
            "Send an operator message to a live run, session or agentiflow.\n\n`id` is the root's id (its prefix picks run, session or agentiflow); `body` is the text, `to` the recipient (default: the root's default), `urgent` interrupts a running turn. Returns the message id and its delivery.",
        )
        .query::<messages::HostQuery>()
        .body::<messages::PostBody>(),
    ]
}
