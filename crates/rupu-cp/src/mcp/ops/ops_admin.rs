//! The operations-admin operator tools: cron triggers and housekeeping.

use super::{op, OpSpec};
use crate::admin_ops::HostQuery;
use crate::api::{cron, maintenance};
use http::Method;

pub(super) fn ops() -> Vec<OpSpec> {
    vec![
        op(
            "cron.list",
            Method::GET,
            "/api/cron",
            "List cron-triggered workflows and when each fires next (rupu cron list).\n\nLocal to this control plane. Returns the `cron_list` report: rows (name, schedule, next_utc, in_seconds; both null when the schedule can't be computed).",
        )
        .query::<HostQuery>(),
        op(
            "cron.events",
            Method::GET,
            "/api/cron/events",
            "List event-triggered workflows, their poll sources and cursors (rupu cron events).\n\nLocal to this control plane. Returns the `cron_events` report: rows (name, event, sources, cursor) and `summary.poll_sources`.",
        )
        .query::<HostQuery>(),
        op(
            "cron.tick",
            Method::POST,
            "/api/cron/tick",
            "Run the cron and event-trigger tick now, the one cp serve runs on a timer (rupu cron tick).\n\n`dry_run` reports what would fire (200 with `would_fire`) and changes nothing; `skip_events` / `only_events` pick one tier. A real tick fires due workflows in the background: 202 `{started}`. 409 while a tick is already running.",
        )
        .query::<HostQuery>()
        .body::<cron::CronTickBody>()
        .open_world(),
        op(
            "maintenance.prune",
            Method::POST,
            "/api/maintenance/prune",
            "Prune old archived transcripts, sessions or netflow ledgers (the CLI's prune and cleanup).\n\n`kind` is transcripts|sessions|netflow|cleanup; `older_than` (`30d`, `12h`) overrides the retention. `dry_run` defaults to true: it returns `would_remove`; pass false to delete, which returns `removed`. 400 for a bad duration.",
        )
        .query::<HostQuery>()
        .body::<maintenance::PruneBody>()
        .destructive(),
    ]
}
