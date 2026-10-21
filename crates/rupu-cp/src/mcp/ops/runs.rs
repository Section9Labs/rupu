//! The runs operator tools: workflow run lists, details and run control.

use super::{op, OpSpec};
use crate::api::{
    autoflow_runs, board, events, run_streams, runs, source, transcript, transcripts,
};
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
            "Approve a workflow run's parked gate so the run continues.\n\n`id` is the run id; `gate` names the gate step when several are parked; `mode` (ask|bypass|readonly) sets the permission mode the resumed run uses. Answers once the approval is recorded, not when the run has resumed. Returns the run detail (`{run, steps, usage}`) plus `host_id`; with a remote `host`, `{ok, host_id}`.",
        )
        .query::<runs::RunControlQuery>()
        .body::<runs::ApproveBody>(),
        op(
            "runs.reject",
            Method::POST,
            "/api/runs/:id/reject",
            "Reject a workflow run's parked gate; the run's rejection path runs.\n\n`id` is the run id; `gate` names the gate step when several are parked; `reason` is recorded with the decision. Answers once the rejection is recorded. Returns the run detail (`{run, steps, usage}`) plus `host_id`; with a remote `host`, `{ok, host_id}`.",
        )
        .query::<runs::RunControlQuery>()
        .body::<runs::RejectBody>()
        .destructive(),
        op(
            "runs.cancel",
            Method::POST,
            "/api/runs/:id/cancel",
            "Cancel a running or paused workflow run.\n\n`id` is the run id; `reason` is recorded. A finished run is refused with HTTP 409. Answers once the cancel is recorded. Returns the run detail (`{run, steps, usage}`) plus `host_id`; with a remote `host`, `{ok, host_id}`.",
        )
        .query::<runs::RunControlQuery>()
        .body::<runs::CancelBody>()
        .destructive(),
        op(
            "runs.list_all",
            Method::GET,
            "/api/runs",
            "List every orchestrator run on any host, whatever started it (by hand, an event, cron or an autoflow).\n\nPage with `offset`/`limit`; `host` restricts to one host (absent fans out), `customer` is a slug or `none`. Not standalone agent runs (see agent_runs.list); for hand-started runs only, see runs.list. Returns a JSON array of run rows.",
        )
        .query::<runs::RunsListQuery>(),
        op(
            "runs.archived",
            Method::GET,
            "/api/runs/archived",
            "List archived workflow runs on this host, newest first.\n\n`kind=workflow` keeps only hand-started runs. Returns a JSON array of run rows.",
        )
        .query::<runs::ArchivedQuery>(),
        op(
            "runs.delete",
            Method::DELETE,
            "/api/runs/:id",
            "Delete a workflow run and its records for good.\n\n`id` is the run id; `host` picks a remote host. A run that has not finished is refused with HTTP 409. Returns `{ok, id, deleted}`.",
        )
        .query::<runs::RunControlQuery>(),
        op(
            "runs.archive",
            Method::POST,
            "/api/runs/:id/archive",
            "Archive a finished workflow run, moving it out of the active lists.\n\n`id` is the run id; `host` picks a remote host. A run that has not finished is refused with HTTP 409. Returns `{ok, id, archived}` (plus `host_id` with a remote `host`). Undo with runs.restore.",
        )
        .query::<runs::RunControlQuery>(),
        op(
            "runs.restore",
            Method::POST,
            "/api/runs/:id/restore",
            "Restore an archived workflow run to the active lists.\n\n`id` is the run id; `host` picks a remote host. Returns `{ok, id, archived}` (plus `host_id` with a remote `host`).",
        )
        .query::<runs::RunControlQuery>(),
        op(
            "runs.pause",
            Method::POST,
            "/api/runs/:id/pause",
            "Pause a running workflow run.\n\n`id` is the run id; `host` picks a remote host. The pause travels by signal to the run's process, so a 200 means it was recorded, not that the run has stopped; read runs.get for the status. Returns the run detail (`{run, steps, usage}`) plus `host_id`; with a remote `host`, `{ok, host_id}`. Undo with runs.resume.",
        )
        .query::<runs::RunControlQuery>(),
        op(
            "runs.resume",
            Method::POST,
            "/api/runs/:id/resume",
            "Resume a paused workflow run, continuing what it left mid-flight.\n\n`id` is the run id; `host` picks a remote host. Any run that is not paused is refused with HTTP 409. The resume is picked up by a worker, so a 200 means it was requested, not that the run is running. Returns the run detail (`{run, steps, usage}`) plus `host_id`; with a remote `host`, `{ok, host_id}`.",
        )
        .query::<runs::RunControlQuery>()
        .open_world(),
        op(
            "runs.graph",
            Method::GET,
            "/api/runs/:id/graph",
            "Get a workflow run's step graph with each node's live state.\n\n`id` is the run id; `host` picks a remote host. Returns the nodes, edges and per-step status.",
        )
        .query::<runs::RunDetailQuery>(),
        op(
            "runs.children",
            Method::GET,
            "/api/runs/:id/children",
            "List the child runs a run launched (sub-agents, agent and workflow children), at any depth.\n\n`id` is the run id; local runs only. Returns `{children}`, each with its handle, kind, status and tokens.",
        ),
        op(
            "runs.board",
            Method::GET,
            "/api/runs/:id/board",
            "Read a run's board: the claims and directives in its message space.\n\n`id` is the run id; `host` picks a remote host. Returns `{claims, directives, journal}`: every claim the board's journal recorded (key, owner, acquired, lease expiry, ended, state held|released|expired), each directive with whether and when it was retracted, and whether the board keeps a journal (without one, only live claims are known).",
        )
        .query::<board::BoardQuery>(),
        op(
            "runs.usage",
            Method::GET,
            "/api/runs/:id/usage",
            "Get a workflow run's live token and cost usage.\n\n`id` is the run id; `host` picks a remote host. `since` and `epoch` (both from a previous answer) return only the points after the ones you hold. Returns the summary, per-step usage and the point series.",
        )
        .query::<runs::RunUsageQuery>(),
        op(
            "runs.usage_timeline",
            Method::GET,
            "/api/runs/:id/usage-timeline",
            "Get a workflow run's usage over time, one series per step.\n\n`id` is the run id; `host` picks a remote host. Returns the timeline points labeled by step.",
        )
        .query::<runs::RunDetailQuery>(),
        op(
            "runs.source",
            Method::GET,
            "/api/runs/:id/source",
            "Read a window of a source file in the run's workspace.\n\n`id` is the run id; `path` is relative to the run's workspace; `line` (1-based, default 1) and `context` (lines either side, default 20) pick the window. Local runs only. Returns the lines and the file's total line count.",
        )
        .query::<source::SourceQuery>(),
        op(
            "runs.ast",
            Method::GET,
            "/api/runs/:id/ast",
            "Get the syntax-tree node at a position in a file of the run's workspace.\n\n`id` is the run id; `path` is relative to the run's workspace; `line` and `col` (1-based, default 1) pick the position. Local runs only. Returns the enclosing node and its kind.",
        )
        .query::<source::AstQuery>(),
        op(
            "runs.autoflow",
            Method::GET,
            "/api/runs/:id/autoflow",
            "Get the autoflow cycle and issue that started a workflow run.\n\n`id` is the run id. Returns the cycle, claim and prior cycles for the same issue; HTTP 404 for a run no autoflow started.",
        ),
        op(
            "agent_runs.list",
            Method::GET,
            "/api/runs/agents",
            "List agent runs (standalone and workflow-step), newest first.\n\nFilter with `q` (e.g. `source:standalone`), `lifecycle` (active|completed|failed), `since`/`until`, `host` (`local`, a host id, or absent for all), `customer`; page with `offset`/`limit`; `facets=1` answers `{rows, facets, facets_partial}`. Not workflow runs (see runs.list). Returns a JSON array of agent run rows.",
        )
        .query::<run_streams::AgentRunsQuery>(),
        op(
            "transcripts.get",
            Method::GET,
            "/api/transcript",
            "Read one run or session transcript as events.\n\n`path` is the transcript file (from a run or session row); `host` picks a remote host, and `run` (the run that recorded `path`) is needed for a remote transcript not yet mirrored. Returns the transcript's events; `partial` is set when a remote host could not be reached to finish collecting.",
        )
        .query::<transcript::PathQ>(),
        op(
            "transcripts.archive",
            Method::POST,
            "/api/transcripts/:id/archive",
            "Archive a standalone agent run's transcript.\n\n`id` is the agent run id; `host` picks a remote host; `ignore_liveness` archives a run whose recorded process id looks alive (a reused pid). Returns `{ok, id}` (plus `host_id` with a remote `host`).",
        )
        .query::<transcripts::TranscriptHostQuery>(),
        op(
            "transcripts.delete",
            Method::DELETE,
            "/api/transcripts/:id",
            "Delete a standalone agent run's transcript for good.\n\n`id` is the agent run id; `host` picks a remote host; `ignore_liveness` deletes a run whose recorded process id looks alive (a reused pid). Returns `{ok, id}` (plus `host_id` with a remote `host`).",
        )
        .query::<transcripts::TranscriptHostQuery>(),
        op(
            "events.recent",
            Method::GET,
            "/api/events",
            "List recent workflow events across runs, newest first (the history behind the live feed).\n\n`limit` pages (default 200); `q` filters with the query line (`run:`, `project:`, `workflow:` and more). To continue, pass the last row's `ts`, `run_id` and `pos` as `before_ts`, `before_run` and `before_pos`. Returns an array of events, each with `ts` and `pos`.",
        )
        .query::<events::RecentEventsQuery>(),
        op(
            "autoflow_runs.list",
            Method::GET,
            "/api/runs/autoflows/runs",
            "List runs started by autoflows, newest first.\n\nFilter with `q`, `lifecycle` (active|completed|failed), `since`/`until`, `host`, `customer`; page with `offset`/`limit`; `facets=1` answers `{rows, facets, facets_partial}`. Returns a JSON array of run rows.",
        )
        .query::<autoflow_runs::AutoflowRunsQuery>(),
        op(
            "autoflow_runs.problems",
            Method::GET,
            "/api/runs/autoflows/problems",
            "List autoflow cycles that failed before they could name a workflow, seen in the last hour.\n\n`host` restricts to one host. Returns the problems with their last error.",
        )
        .query::<autoflow_runs::ProblemsQuery>(),
        op(
            "autoflow_runs.cycles",
            Method::GET,
            "/api/runs/autoflows",
            "List autoflow cycle records (the per-poll history), newest first.\n\n`host` is `local`, a host id, or absent for all; `since`/`until` bound the time; page with `offset`/`limit`. Returns a JSON array of cycles.",
        )
        .query::<run_streams::AutoflowRunsQuery>(),
        op(
            "autoflow_runs.events",
            Method::GET,
            "/api/runs/autoflows/events",
            "List autoflow cycle events (claimed, dispatched, failed and so on), newest first.\n\n`host` is `local`, a host id, or absent for all; `since`/`until` bound the time; page with `offset`/`limit`. Returns a JSON array of events.",
        )
        .query::<run_streams::AutoflowEventsQuery>(),
        // Plan 3a, package B: retry and continue.
        op(
            "runs.retry",
            Method::POST,
            "/api/runs/:id/retry",
            "Retry a failed, rejected or cancelled workflow run (`rupu workflow resume`).\n\n`id` is the run id; `restart_interrupted` starts interrupted steps and units over instead of continuing them; `mode` (ask|bypass|readonly) overrides the run's own launch mode, which it keeps otherwise. A run left running by a dead runner is marked failed first and retried. Answers 202 `{run_id, retrying, restart_interrupted, host_id}` once the resume is spawned: watch the run's status. 409 for a running, paused (use runs.resume), awaiting-approval or completed run, and for a standalone agent run (use agent_runs.continue); local-only (a remote `host` is a 501).",
        )
        .query::<runs::RunControlQuery>()
        .body::<runs::RetryBody>()
        .open_world(),
        op(
            "agent_runs.continue",
            Method::POST,
            "/api/runs/agents/:id/continue",
            "Continue a standalone agent run, or a sub-agent's run, from its transcript (`rupu run --continue`).\n\n`id` is a `run_…` or `sub_…` id. The continuation restores every flag the run was launched with and runs in its directory; `model`, `provider`, `mode`, `findings_profile`, `engagement_profiles`, `skills`, `preload_skills` and `messaging` replace the launch's own (an empty list keeps it). `prompt` (optional, not blank) is what the run is told next: with one, ANY ended run carries on — an interrupted run gets the interruption note then the prompt, a finished run gets the prompt as a new turn (a real model call), a failed run continues on the same model. Without one, an interrupted run carries on, a finished run answers with its recorded answer and calls no model, and a failed run continues only on a different `model`/`provider`. Answers 202 `{run_id, continues, host_id}` with the new run's id. 409 while the run is still running; local-only (a remote `host` is a 501).",
        )
        .query::<runs::RunControlQuery>()
        .body::<run_streams::ContinueAgentRunBody>()
        .open_world(),
    ]
}
