//! Operator work queue tools. Agent pickup and result tools live in rupu-tools.
use super::{op, OpSpec};
use crate::api::board_work;
use http::Method;

pub(super) fn ops() -> Vec<OpSpec> {
    vec![
        op("board.work.list", Method::GET, "/api/{root}/:id/board/work",
            "Read a bounded page of durable Board work for a local owner.\n\n`id` selects a session, run or agentiflow by prefix; host must be local. Optional state and claimant_run_id filters are bound into next_cursor; limit is 1 through 200, default 50. Returns owner actions, queue identity/revision, capacity, summaries and counts matching the filters at that revision. The 1 MiB byte budget can reduce the page. A 409 cursor conflict requires a refresh. Reading does not launch work.")
            .query::<board_work::ListQuery>(),
        op("board.work.get", Method::GET, "/api/{root}/:id/board/work/:work_id",
            "Read one current Board work item and its permitted operator actions.\n\n`id` is the local owner and work_id the immutable item ID. Returns a bounded current item, latest attempt/result, source availability and capacity. Historical attempts and results are in board.work.timeline. A recorded result is the executor's report; transcript availability is separate.")
            .query::<board_work::HostQuery>(),
        op("board.work.timeline", Method::GET, "/api/{root}/:id/board/work/:work_id/timeline",
            "Read a bounded immutable event page for one Board work item.\n\n`id` is the local owner; work_id identifies the item. Events are oldest first; limit is 1 through 200, default 50. Follow next_cursor with the same owner and item. Counts cover all matching events at source_revision. Exact recorded execution and invocation provenance remain present when transcript bytes are unavailable.")
            .query::<board_work::TimelineQuery>(),
        op("board.work.post", Method::POST, "/api/{root}/:id/board/work",
            "Post durable Board work as the authenticated operator.\n\nSupply owner_token, request_id, expected_queue_id (null only for an uninitialized queue), expected_revision and operation {action:post,title,instructions}. Expected revision is the queue revision. Identical retries use the same request ID; retain it for uncertain outcomes. The receipt reports execution_effect:none: posting does not launch or assign a worker.")
            .query::<board_work::HostQuery>().body::<board_work::PostRequest>().idempotent(),
        op("board.work.transitions", Method::POST, "/api/{root}/:id/board/work/:work_id/transitions",
            "Cancel or requeue Board work as the authenticated operator.\n\nSupply owner_token, request_id, expected_queue_id, expected_revision and operation {action:cancel|requeue,reason}. Expected revision is the item revision. Neither action stops an executor; prior_attempt_may_still_run preserves that uncertainty. Requeue keeps prior attempts and results. Operator tools cannot claim, choose an executor or submit a worker result.")
            .query::<board_work::HostQuery>().body::<board_work::TransitionRequest>().idempotent(),
    ]
}
