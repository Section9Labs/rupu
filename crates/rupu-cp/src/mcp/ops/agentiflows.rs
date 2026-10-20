//! The agentiflows operator tools.

use super::{op, OpSpec};
use crate::api::{agentiflow_defs, agentiflows};
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
        op(
            "agentiflows.run",
            Method::POST,
            "/api/agentiflows",
            "Launch an agentiflow (a goal-directed fleet run) from a definition, in the background.\n\n`definition` names the definition; `scope_kind` (global|project) with `scope_id`, or an absolute `working_dir`, picks where it starts. The definition is checked first (404 unknown, 400 invalid). Returns 202 `{id, status}`: `started` once the run has recorded itself, or `pending` with the `log` it writes to. Follow it with `agentiflows.get` or `wait {agentiflow}`.",
        )
        .query::<agentiflows::LocalHostQuery>()
        .body::<agentiflows::LaunchRequest>()
        .open_world(),
        op(
            "agentiflows.stop",
            Method::POST,
            "/api/agentiflows/:id/stop",
            "Stop a running agentiflow, gracefully or (with `now`) at once.\n\n`id` is the agentiflow id. Without `now` the stop is queued for the next round boundary: `{queued: true}`. With `now` the coordinator and its units are signalled and the run is recorded failed (`operator_stop:now`): `{stopped, status, stop_reason}` once done. 409 when the flow is not running.",
        )
        .query::<agentiflows::LocalHostQuery>()
        .body::<agentiflows::StopRequest>()
        .destructive(),
        op(
            "agentiflow_defs.list",
            Method::GET,
            "/api/agentiflow-defs",
            "List agentiflow definitions in the global layer and every registered project.\n\nTakes no arguments (`host` can only be `local`). Returns a JSON array of `{name, scope, scope_kind, scope_id, description, lead, engagement_profiles, error}`; a project definition hides a same-named global one, and `error` says why a file does not parse.",
        )
        .query::<agentiflows::LocalHostQuery>(),
        op(
            "agentiflow_defs.get",
            Method::GET,
            "/api/agentiflow-defs/:name",
            "Read one agentiflow definition.\n\n`name` is the definition's file stem; `scope_kind` (global|project) with `scope_id` picks the layer, else the project-first match. Returns `{name, def, yaml, error, scope, scope_kind, scope_id}`: the parsed definition (null when it does not parse) and its raw YAML.",
        )
        .query::<agentiflow_defs::DefQuery>(),
        op(
            "agentiflow_defs.create",
            Method::POST,
            "/api/agentiflow-defs",
            "Create a new agentiflow definition in the global layer.\n\n`raw` is the whole YAML file; the name comes from its `name:`. Returns the new definition as `agentiflow_defs.get` does. 400 if it does not parse or is named `validate`, 409 if a definition of that name exists in any layer.",
        )
        .query::<agentiflows::LocalHostQuery>()
        .body::<agentiflow_defs::DefWriteBody>(),
        op(
            "agentiflow_defs.save",
            Method::PUT,
            "/api/agentiflow-defs/:name",
            "Overwrite an agentiflow definition with a new file.\n\n`raw` is the whole YAML file and its `name:` must equal `name`. `scope_kind` (global|project) with `scope_id` picks the layer; without them the project-first match is overwritten, or a new global file is made. Returns the saved definition as `agentiflow_defs.get` does.",
        )
        .query::<agentiflow_defs::DefQuery>()
        .body::<agentiflow_defs::DefWriteBody>(),
        op(
            "agentiflow_defs.delete",
            Method::DELETE,
            "/api/agentiflow-defs/:name",
            "Delete an agentiflow definition file.\n\n`scope_kind` (global|project) with `scope_id` picks the layer; without them the project-first match is deleted. Returns `{deleted, scope, scope_kind}`; 404 if nothing matches.",
        )
        .query::<agentiflow_defs::DefQuery>(),
        op(
            "agentiflow_defs.validate",
            Method::POST,
            "/api/agentiflow-defs/validate",
            "Check an agentiflow definition the way `agentiflow run` does before it starts, without saving it.\n\n`raw` is the whole YAML file; `scope_id` adds that project's engagement profiles. Returns `{ok: true, name, lead, engagement_profiles}`, or HTTP 400 naming the first problem (parse, unknown profile, or a validation rule).",
        )
        .query::<agentiflows::LocalHostQuery>()
        .body::<agentiflow_defs::ValidateBody>()
        .read_only(),
    ]
}
