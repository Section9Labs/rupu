//! The setup operator tools: the stock fleet, project init and the
//! engagement profiles a launch can select.

use super::{op, OpSpec};
use crate::api::{engagement_profiles, fleet};
use http::Method;

pub(super) fn ops() -> Vec<OpSpec> {
    vec![
        op(
            "fleet.list",
            Method::GET,
            "/api/fleet",
            "List the stock security-assessment fleet and where it is installed.\n\n`ws_id` also reports that project's `.rupu/`. Returns `{entries, global_root, project_root}`: per agent or workflow its `kind`, `name`, `path` and status `global` (and `project`) — `missing`, `current` (the shipped file), `modified` or `unreadable`.",
        )
        .query::<fleet::FleetQuery>(),
        op(
            "fleet.install",
            Method::POST,
            "/api/fleet/install",
            "Install the stock fleet's agents and workflows into the global rupu root or a project.\n\n`project` (a project id) installs into its `.rupu/`; `force` overwrites fleet files that exist (by default they are kept). Returns `{root, created, skipped, overwrote, files}`, `files` one `{path, action}` per file.",
        )
        .query::<fleet::LocalQuery>()
        .body::<fleet::FleetInstallBody>()
        .destructive(),
        op(
            "projects.init",
            Method::POST,
            "/api/projects/init",
            "Initialise a directory as a rupu project: `.rupu/` skeleton, config, gitignore entries.\n\n`path` must be an absolute, existing directory; `with_samples` adds the curated agent and workflow samples; `force` re-runs on an initialised project, overwriting samples (never `config.toml`). Returns `{root, created, skipped, overwrote, files}`; `HTTP 409` when already initialised without `force`.",
        )
        .query::<fleet::LocalQuery>()
        .body::<fleet::InitBody>(),
        op(
            "engagement_profiles.list",
            Method::GET,
            "/api/engagement-profiles",
            "List the engagement profiles a launch can select, built-in and layered.\n\n`ws_id` overlays that project's `.rupu/profiles`. Returns `{profiles, errors}`: each profile with its `layer` (builtin, global, project), bundle, asset kinds, completeness checks and coverage; `errors` are profile files that don't load, which fail every engagement launch.",
        )
        .query::<engagement_profiles::ProfilesQuery>(),
        op(
            "engagement_profiles.get",
            Method::GET,
            "/api/engagement-profiles/:id",
            "Get one engagement profile: its bundle, asset kinds and completeness checks.\n\n`id` is the profile id; `ws_id` overlays that project's `.rupu/profiles`. Returns the profile with its `layer` and the overlay's `errors`; `HTTP 404` for an unknown id.",
        )
        .query::<engagement_profiles::ProfilesQuery>(),
    ]
}
