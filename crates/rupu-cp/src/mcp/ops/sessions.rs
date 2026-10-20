//! The sessions operator tools, and the message log every run, session and
//! agentiflow has.

use super::{op, OpSpec};
use crate::api::{agents, messages, sessions};
use http::Method;

pub(super) fn ops() -> Vec<OpSpec> {
    vec![
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
