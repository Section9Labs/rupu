//! The runs operator tools: workflow run lists, details and run control.

use super::{op, OpSpec};
use crate::api::runs;
use http::Method;

pub(super) fn ops() -> Vec<OpSpec> {
    vec![
        op(
            "runs.list",
            Method::GET,
            "/api/runs/workflows",
            "List workflow runs started by hand (not standalone agent runs, not event- or cron-started ones), newest first.\n\nFilter with `q` (the query line, e.g. `status:running`), `lifecycle` (active|completed|failed), `since`/`until`, `host`, and page with `offset`/`limit`. Returns a JSON array of run rows; with `facets` set, an object `{rows, facets, facets_partial}`.",
        )
        .query::<runs::WorkflowRunsQuery>(),
        op(
            "runs.get",
            Method::GET,
            "/api/runs/:id",
            "Get one workflow run's full record.\n\n`id` is the run id; `host` picks a remote host. Returns the run detail: status, inputs, steps, any parked gate.",
        )
        .query::<runs::RunDetailQuery>(),
        op(
            "runs.approve",
            Method::POST,
            "/api/runs/:id/approve",
            "Approve a workflow run's parked gate so the run continues.\n\n`id` is the run id; `gate` names the gate step when several are parked; `mode` (ask|bypass|readonly) sets the permission mode the resumed run uses. Returns once the approval is recorded, not when the run has resumed.",
        )
        .query::<runs::RunControlQuery>()
        .body::<runs::ApproveBody>(),
        op(
            "runs.reject",
            Method::POST,
            "/api/runs/:id/reject",
            "Reject a workflow run's parked gate; the run's rejection path runs.\n\n`id` is the run id; `gate` names the gate step when several are parked; `reason` is recorded with the decision. Returns once the rejection is recorded.",
        )
        .query::<runs::RunControlQuery>()
        .body::<runs::RejectBody>()
        .destructive(),
        op(
            "runs.cancel",
            Method::POST,
            "/api/runs/:id/cancel",
            "Cancel a running or paused workflow run.\n\n`id` is the run id; `reason` is recorded. A finished run is refused with HTTP 409. Returns once the cancel is recorded.",
        )
        .query::<runs::RunControlQuery>()
        .body::<runs::CancelBody>()
        .destructive(),
    ]
}
