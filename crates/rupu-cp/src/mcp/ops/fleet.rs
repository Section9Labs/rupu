//! The fleet operator tools: hosts and workers, config writes, customers,
//! the launch preview and repos.

use super::{op, OpSpec};
use crate::api::{config, customers, hosts, launch_preview};
use http::Method;

pub(super) fn ops() -> Vec<OpSpec> {
    vec![
        op(
            "hosts.list",
            Method::GET,
            "/api/hosts",
            "List the registered hosts with their last-probed health.\n\nTakes no arguments. Returns a JSON array of `{id, name, transport_kind, base_url?, status, version?, capabilities?, active_run_count, last_seen_at?}`; the local host is first and always online, a remote's `status` is `online` or `offline` from its last probe (served from a cache, refreshed in the background).",
        ),
        op(
            "hosts.registered",
            Method::GET,
            "/api/hosts/registered",
            "List the registered hosts without probing any of them.\n\nTakes no arguments. Returns a JSON array of `{id, name, transport_kind}` read from the store only, local first; `transport_kind` is local, http_cp, tunnel, ssh or bucket.",
        ),
        op(
            "hosts.add",
            Method::POST,
            "/api/hosts",
            "Register a remote control plane as an HTTP host.\n\n`name` and `base_url` are required; `token` is the remote's bearer token. Returns the new host as `{id, name, transport_kind, base_url, status, active_run_count, ...}` with `status` offline until probed; `HTTP 501` on a read-only deploy.",
        )
        .body::<hosts::AddHostBody>(),
        op(
            "hosts.add_ssh",
            Method::POST,
            "/api/hosts/ssh",
            "Register an SSH host.\n\n`name` and `host` are required; `port` and `identity_file` (a path on this machine, metadata only) are optional, and no secret is stored. Returns the new host as `{id, name, transport_kind, base_url, status, active_run_count, ...}` with `status` offline until probed; `HTTP 501` on a read-only deploy.",
        )
        .body::<hosts::AddSshHostBody>(),
        op(
            "hosts.add_bucket",
            Method::POST,
            "/api/hosts/bucket",
            "Register a bucket (dead-drop) host.\n\n`name` and `url` are required; `prefix` is an optional key prefix, and credentials come from the environment, none are stored. Returns the new host as `{id, name, transport_kind, base_url, status, active_run_count, ...}` with `status` offline until probed; `HTTP 501` on a read-only deploy.",
        )
        .body::<hosts::AddBucketHostBody>(),
        op(
            "hosts.enroll_node",
            Method::POST,
            "/api/hosts/node",
            "Enroll a tunnel node and mint its one-time token.\n\n`name` is required. Returns `{host, command, token}`: the new host (status offline until the node connects), the `rupu node` command to run on the target machine (dialing this server's `Host`, or its routable IP when that is loopback) and the plaintext token, shown only in this response; `HTTP 400` for an empty name, `HTTP 501` on a read-only deploy.",
        )
        .body::<hosts::EnrollNodeBody>(),
        op(
            "hosts.remove",
            Method::DELETE,
            "/api/hosts/:id",
            "Remove a registered host.\n\n`id` is the host id; a tunnel host's live connection is closed. Returns `{status: 204}` on success; `HTTP 400` for `local`, which cannot be removed, and `HTTP 404` for an unknown host.",
        ),
        op(
            "workers.list",
            Method::GET,
            "/api/workers",
            "List the local workers with the runs attributed to each.\n\nTakes no arguments. Returns a JSON array of worker records (`worker_id`, `kind`, `name`, `host`, `capabilities`, `registered_at`, `last_seen_at`) each with `active_run_count`, `total_run_count` and `last_run_at`.",
        ),
        op(
            "host.info",
            Method::GET,
            "/api/host/info",
            "Describe this control plane as a host.\n\nTakes no arguments. Returns `{version, capabilities, features}`: `capabilities` is `{backends, scm_hosts, permission_modes}` aggregated over the local workers, `features` the list of request fields and endpoints this build honours.",
        ),
        op(
            "config.get",
            Method::GET,
            "/api/config",
            "Read the effective config, its provenance and the raw TOML of each layer.\n\n`project` (a project id) or `customer` (a slug) layers that scope over global; give at most one, both is `HTTP 400`. Returns `{effective, provenance, raw_global, raw_project, raw_customer, customer, customer_lock, layer_error, layer_error_kept?, cp, status}`; a malformed layer is reported in `layer_error` and the view still comes back.",
        )
        .query::<config::ConfigQuery>(),
        op(
            "config.put_global",
            Method::PUT,
            "/api/config/global",
            "Write the global config.\n\nGive `raw` (the whole TOML text, which wins when both are given) or `patch` (an object of dotted keys merged onto the current file); the result is validated before anything is written. Returns `{ok: true, restart_required}`, the start-time-only keys whose saved value differs from the running one; `HTTP 400` for invalid config, `HTTP 501` on a read-only deploy.",
        )
        .body::<config::ConfigWriteBody>()
        .destructive(),
        op(
            "config.put_customer",
            Method::PUT,
            "/api/config/customer/:slug",
            "Write one customer's config layer.\n\n`slug` is the customer; `raw` (the whole TOML text, which wins when both are given) or `patch` (dotted keys merged onto the layer; `{\"policy.lock\": [...]}` sets the customer's lock list). The layer is validated on top of global and keys the global `[policy].lock` enforces are refused. Returns `{ok: true}`; `HTTP 400` for invalid or locked config, `HTTP 404` for an unknown customer, `HTTP 501` on a read-only deploy.",
        )
        .body::<config::ConfigWriteBody>()
        .destructive(),
        op(
            "config.put_project",
            Method::PUT,
            "/api/config/project/:id",
            "Write one project's config layer.\n\n`id` is the project id; `raw` (the whole TOML text, which wins when both are given) or `patch` (dotted keys merged onto the layer). Keys locked by the global or the project's customer `[policy].lock` are refused. Returns `{ok: true}`; `HTTP 400` for invalid or locked config, or a project whose config is the global config, `HTTP 404` for an unknown project, `HTTP 501` on a read-only deploy.",
        )
        .body::<config::ConfigWriteBody>()
        .destructive(),
        op(
            "config.put_policy",
            Method::PUT,
            "/api/config/policy",
            "Set the global list of locked config keys.\n\n`lock` is the complete list of dotted keys that project and customer layers may no longer set; it replaces the current list. Returns `{ok: true}`; `HTTP 400` for invalid config, `HTTP 501` on a read-only deploy.",
        )
        .body::<config::PolicyBody>()
        .destructive(),
        op(
            "customers.list",
            Method::GET,
            "/api/customers",
            "List the customers with a usage rollup.\n\n`range` is 7d, 30d or all; `archived` is `1` or `true` to include archived customers. Returns a JSON array of `{slug, name, notes, contact, color, tint, archived, created_at, rollup, default_account, layer_error}`, where `rollup` covers the projects, runs, usage, open findings and last activity in the range.",
        )
        .query::<customers::RangeQuery>(),
        op(
            "customers.create",
            Method::POST,
            "/api/customers",
            "Create a customer.\n\n`slug` and `name` are required; `notes`, `contact` and `color` are optional. Returns the new customer as `{slug, name, notes, contact, color, tint, archived, created_at}` with `HTTP 201`; a taken or invalid slug is an error.",
        )
        .body::<customers::CreateBody>(),
        op(
            "customers.get",
            Method::GET,
            "/api/customers/:slug",
            "Read one customer with its rollup and projects.\n\n`slug` is the customer; `range` is 7d, 30d or all, and `archived` is accepted as for customers.list. Returns `{customer, rollup, projects, default_account, layer_error, config_path}`, each project row carrying only this customer's work in the range; `HTTP 404` for an unknown customer.",
        )
        .query::<customers::RangeQuery>(),
        op(
            "customers.update",
            Method::PATCH,
            "/api/customers/:slug",
            "Change a customer's name, notes, contact or color.\n\n`slug` is the customer; fields left out stay as they are and an empty string clears `notes`, `contact` or `color`. Returns the customer as `{slug, name, notes, contact, color, tint, archived, created_at}`; `HTTP 404` for an unknown customer.",
        )
        .body::<customers::PatchBody>(),
        op(
            "customers.delete",
            Method::DELETE,
            "/api/customers/:slug",
            "Delete a customer.\n\n`slug` is the customer. Returns `{status: 204}` on success; while projects are still assigned the call is an error, `HTTP 409` with `{error, projects: [{ws_id, path}]}`.",
        ),
        op(
            "customers.archive",
            Method::POST,
            "/api/customers/:slug/archive",
            "Archive a customer.\n\n`slug` is the customer. Returns the customer as `{slug, name, notes, contact, color, tint, archived, created_at}` with `archived` true; `HTTP 404` for an unknown customer.",
        ),
        op(
            "customers.unarchive",
            Method::POST,
            "/api/customers/:slug/unarchive",
            "Unarchive a customer.\n\n`slug` is the customer. Returns the customer as `{slug, name, notes, contact, color, tint, archived, created_at}` with `archived` false; `HTTP 404` for an unknown customer.",
        ),
        op(
            "customers.assign_project",
            Method::PUT,
            "/api/customers/:slug/projects/:ws_id",
            "Assign a project to a customer, replacing any earlier assignment.\n\n`slug` is the customer and `ws_id` the project id. Returns the project's row (`ws_id`, `name`, `path`, usage, `run_count`, `customer`, ...); `HTTP 404` for an unknown customer or project.",
        ),
        op(
            "customers.unassign_project",
            Method::DELETE,
            "/api/customers/:slug/projects/:ws_id",
            "Remove a project from a customer.\n\n`slug` is the customer and `ws_id` the project id. Returns `{status: 204}`; `HTTP 404` when the customer is unknown or the project is not assigned to it.",
        ),
        op(
            "launch.preview",
            Method::POST,
            "/api/launch/preview",
            "Preview which customer and accounts a launch would use.\n\nName exactly one of `workflow` or `agent`; `scope_kind` and `scope_id` pick the project as a launch does and are exclusive with `working_dir`. `host` is echoed, and a non-local one adds a warning, because the preview always resolves against this machine. Returns `{customer, accounts, warnings, host?}`; `HTTP 400` for an invalid request.",
        )
        .body::<launch_preview::PreviewBody>()
        .read_only(),
        op(
            "repos.list",
            Method::GET,
            "/api/repos",
            "List the repositories reachable through the configured SCM accounts.\n\nTakes no arguments. Returns a JSON array of `{platform, account, repo, default_branch, private}` across every account; `HTTP 501` when the server has no SCM credentials.",
        )
        .open_world(),
    ]
}
