//! The agentiflows operator tools.

use super::{op, OpSpec};
use crate::api::agentiflows;
use http::Method;

pub(super) fn ops() -> Vec<OpSpec> {
    vec![
        op(
            "agentiflows.list",
            Method::GET,
            "/api/agentiflows",
            "List agentiflows (goal-directed fleet runs), newest first.\n\nTakes no arguments. Returns `{rows}` with each flow's status, rounds, goals and spend.",
        ),
        op(
            "agentiflows.get",
            Method::GET,
            "/api/agentiflows/:id",
            "Get one agentiflow's record.\n\n`id` is the agentiflow id (`af_...`). Returns its status, goals, rounds, units and spend.",
        ),
        op(
            "agentiflows.events",
            Method::GET,
            "/api/agentiflows/:id/events",
            "Read an agentiflow's recorded events.\n\n`id` is the agentiflow id. Returns `{events}`, the recorded events oldest first.",
        ),
        op(
            "agentiflows.children",
            Method::GET,
            "/api/agentiflows/:id/children",
            "List the units an agentiflow has launched.\n\n`id` is the agentiflow id. Returns `{children}`, the child rows.",
        ),
        op(
            "agentiflows.unit_output",
            Method::GET,
            "/api/agentiflows/:id/units/:unit_id/output",
            "Read the output one agentiflow unit produced.\n\n`id` is the agentiflow id, `unit_id` the unit's id. Returns `{output}`: the unit's output text, or null when it has none.",
        ),
        op(
            "agentiflows.steer",
            Method::POST,
            "/api/agentiflows/:id/steer",
            "Send the agentiflow's lead an operator instruction, or ask the flow to stop.\n\n`id` is the agentiflow id; `message` is the instruction, `now` delivers it mid-round, `stop` asks the envelope to wind the run down. Without `stop`, returns 202 `{id, delivery}` for the message sent to the lead; with `stop`, queues the stop and returns `{queued: true}`. A flow that is not live is refused with HTTP 409 on both paths.",
        )
        .body::<agentiflows::SteerRequest>(),
    ]
}
