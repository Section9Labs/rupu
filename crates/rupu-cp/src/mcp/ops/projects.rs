//! The projects operator tools: the project list and rollup, a project's
//! runs, sessions, definitions and coverage, and its code (tree, source,
//! file list).

use super::{op, OpSpec};
use crate::api::{code, fleet, project_scm, projects};
use http::Method;

pub(super) fn ops() -> Vec<OpSpec> {
    vec![
        op(
            "projects.list",
            Method::GET,
            "/api/projects",
            "List the registered projects with their usage rollup and customer.\n\n`customer` (a slug, or `none`) keeps the projects currently assigned to that customer; a bad slug is `HTTP 400`. Returns a JSON array of rows `{ws_id, name, path, repo_remote, branch, repo_home_url?, created_at, last_run_at, usage, run_count, last_active, customer}`.",
        )
        .query::<projects::ProjectsQuery>(),
        op(
            "projects.register",
            Method::POST,
            "/api/projects",
            "Register an existing local project directory without starting work.\n\n`path` must be absolute and name an existing directory. Registration reuses the canonical workspace identity and preserves last-run time. Optional `customer` assigns an existing active customer; omitted or null preserves its current assignment. Returns the normal project row. Remote hosts and read-only deployments are unsupported.",
        )
        .query::<fleet::LocalQuery>()
        .body::<projects::RegisterProjectBody>(),
        op(
            "projects.get",
            Method::GET,
            "/api/projects/:ws_id",
            "Get one project's rollup of runs, sessions and coverage.\n\n`ws_id` is the project id. Returns an object `{project, runs, sessions, coverage, recent_runs, usage}`: `project` is the row `projects.list` gives, `runs` counts total, running, by status and by surface, and `recent_runs` holds the ten newest run rows; `HTTP 404` for an unknown project.",
        ),
        op(
            "projects.scm",
            Method::GET,
            "/api/projects/:ws_id/scm",
            "Inspect a registered checkout's branch, changes and optional current-branch reviews.\n\n`ws_id` is the project ID on the directly connected instance; omit `host` or use local. Returns bounded staged and working diffs, checkout provenance, limits and observed state. The default performs local inspection only. `include_review:true` additionally makes a bounded read through the owning instance's configured GitHub/GitLab account; failures remain unavailable, and unknown checks are not success. No checkout, branch, review or merge mutation is performed.",
        )
        .query::<project_scm::ContextQuery>()
        .open_world(),
        op(
            "projects.runs",
            Method::GET,
            "/api/projects/:ws_id/runs",
            "List a project's workflow runs, newest first.\n\n`q` is the run query line (e.g. `status:running`), applied before paging with `offset`/`limit`; `facets=1` answers `{rows, facets, facets_partial}`. Returns a JSON array of run rows, each with its customer and priced usage; `HTTP 400` for a `q` that does not parse, `HTTP 404` for an unknown project.",
        )
        .query::<projects::ProjectListQuery>(),
        op(
            "projects.sessions",
            Method::GET,
            "/api/projects/:ws_id/sessions",
            "List a project's sessions, newest activity first by default.\n\n`sort=archived_at` orders by archive transition before paging, falling back to activity for legacy records without an archive date.\n\n`q` is the session query line (e.g. `is:active`), applied before paging with `offset`/`limit`; `facets=1` answers `{rows, facets, facets_partial}`. Returns a JSON array of session rows; `HTTP 400` for a `q` that does not parse, `HTTP 404` for an unknown project.",
        )
        .query::<projects::ProjectListQuery>(),
        op(
            "projects.start_session",
            Method::POST,
            "/api/projects/:ws_id/sessions",
            "Start a project-owned conversation with the bundled rupu-session coding harness.\n\nNo agent setup is required. `prompt` is required unless `idle:true` creates a zero-turn session without a provider worker; idle and prompt cannot be combined. Optional `provider` requires `model`, and both are applied before the first turn. `mode` defaults to readonly; bypass is explicit and ask is refused because detached workers have no approval channel. The directly connected host owns execution. Returns {session_id, host_id, agent, provider, model}; acceptance is not provider completion.",
        )
        .query::<crate::api::project_session_start::StartQuery>()
        .body::<crate::api::project_session_start::StartBody>(),
        op(
            "projects.agents",
            Method::GET,
            "/api/projects/:ws_id/agents",
            "List the agents a project can use: its own merged over the global ones.\n\nTakes only `ws_id`. A project agent shadows a global one of the same name. Returns a JSON array of agent rows, each tagged `scope` (`project` or `global`) with a `load_error` when its file or skills do not check out; `HTTP 404` for an unknown project.",
        ),
        op(
            "projects.workflows",
            Method::GET,
            "/api/projects/:ws_id/workflows",
            "List the workflows a project can use: its own merged over the global ones.\n\nTakes only `ws_id`. A project workflow shadows a global one of the same name. Returns a JSON array of workflow rows, each tagged `scope` (`project` or `global`); `HTTP 404` for an unknown project.",
        ),
        op(
            "projects.autoflows",
            Method::GET,
            "/api/projects/:ws_id/autoflows",
            "List the autoflow definitions a project can use, enabled or not.\n\nTakes only `ws_id`. These are workflows carrying an `autoflow:` block, project ones shadowing global ones by name. Returns a JSON array of rows sorted by name; `HTTP 404` for an unknown project.",
        ),
        op(
            "projects.coverage",
            Method::GET,
            "/api/projects/:ws_id/coverage",
            "List a project's coverage targets with their finding counts.\n\nTakes only `ws_id`. Returns a JSON array of `{target_id, assertion_lines, has_catalog, findings}`, read from the project's own path; `HTTP 404` for an unknown project.",
        ),
        op(
            "projects.coverage_assessed",
            Method::GET,
            "/api/projects/:ws_id/coverage/assessed",
            "Get the share of a project's concerns that are fully assessed.\n\nTakes only `ws_id`. Runs the audit of every target, so it is the slow read of the coverage pair. Returns `{assessed_pct}`, a number from 0 to 100, or null when no target has concerns; `HTTP 404` for an unknown project.",
        ),
        op(
            "projects.tree",
            Method::GET,
            "/api/projects/:ws_id/tree",
            "List one directory of a project's files.\n\n`path` is a project-relative directory; omitted, the project root. Noise directories (`.git`, `.rupu`, `node_modules`, `target`) are hidden. Returns `{path, parent, entries}` with `entries` as `{name, path, kind}` (`dir` or `file`); a path that is not a directory or escapes the project is `HTTP 400`, `HTTP 404` for an unknown project.",
        )
        .query::<code::TreeQuery>(),
        op(
            "projects.source",
            Method::GET,
            "/api/projects/:ws_id/source",
            "Read one file of a project as numbered lines.\n\n`path` is a project-relative file. Returns the JSON `{available, path, language, totalLines, lines}` with `lines` as `{n, text}`; a missing, oversized or binary file answers `{available: false, reason}` with status 200, and a path escaping the project is an error; `HTTP 404` for an unknown project.",
        )
        .query::<code::TreeQuery>(),
        op(
            "projects.files",
            Method::GET,
            "/api/projects/:ws_id/files",
            "List every file path of a project.\n\nTakes only `ws_id`. Noise directories are skipped and symlinks are not followed. Returns `{files, truncated}`: sorted project-relative paths, capped at 20,000 with `truncated` true past that; `HTTP 404` for an unknown project.",
        ),
    ]
}
