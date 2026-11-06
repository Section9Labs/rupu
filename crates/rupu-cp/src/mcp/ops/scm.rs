//! The SCM operator tools: accounts and account-selection rules, the local
//! tracked-repo registry and auth status. None of them returns any part of
//! a credential.

use super::{op, OpSpec};
use crate::api::{repos, scm};
use http::Method;

pub(super) fn ops() -> Vec<OpSpec> {
    vec![
        op(
            "scm_accounts.list",
            Method::GET,
            "/api/scm/accounts",
            "List the configured GitHub/GitLab accounts, the rules that select each and whether it has a credential.\n\nRead from the global config; `host` other than `local` is `HTTP 501`. Returns a JSON array of `{account, kind, base_url, rules, credentials, state}`: `kind` github or gitlab, `rules` each `{owner}` or `{path}`, `state` ok, needs_login or absent. Never a token.",
        )
        .query::<scm::LocalHostQuery>(),
        op(
            "scm_rules.list",
            Method::GET,
            "/api/scm/rules",
            "List the account-selection rules (`[[scm.rules]]`) in the global config.\n\nReturns a JSON array of `{owner?, path?, account}` in config order: a repo whose owner matches `owner`, or a run whose working directory matches `path`, uses `account`. `host` other than `local` is `HTTP 501`.",
        )
        .query::<scm::LocalHostQuery>(),
        op(
            "scm_rules.bind",
            Method::POST,
            "/api/scm/rules",
            "Add an account-selection rule to the global config, as `rupu scm bind` does.\n\nGive `account` and exactly one of `owner` (an owner glob, `acme/*`) or `path` (a path glob, `~/Code/work/*`). Returns `{rule, warning?, rules}`; `warning` says the account is not declared, so the rule may never match. `HTTP 400` for a bad rule or a config that would not validate.",
        )
        .query::<scm::LocalHostQuery>()
        .body::<scm::BindBody>()
        .destructive(),
        op(
            "repos.tracked",
            Method::GET,
            "/api/repos/tracked",
            "List the local checkouts tracked in the repo registry.\n\nTakes no arguments. Returns a JSON array of `{id, repo, preferred_path, known_paths, default_branch?, last_seen_at}`; `id` is what `repos.forget` takes, `repo` is `<platform>:<owner>/<repo>`.",
        )
        .query::<scm::LocalHostQuery>(),
        op(
            "repos.attach",
            Method::POST,
            "/api/repos/tracked",
            "Track a local git checkout as a copy of a repo, as `rupu repos attach` does.\n\n`repo` is `github:owner/repo` (run-target syntax) and `path` the checkout's absolute path; its `origin` must point at `repo`. Returns the tracked repo `{id, repo, preferred_path, known_paths, ...}`; `HTTP 400` when the path is no checkout of `repo`.",
        )
        .query::<scm::LocalHostQuery>()
        .body::<repos::TrackBody>(),
        op(
            "repos.prefer",
            Method::POST,
            "/api/repos/tracked/prefer",
            "Make a local checkout the preferred path for a repo, tracking it first if needed (`rupu repos prefer`).\n\n`repo` is `github:owner/repo` and `path` the checkout's absolute path; its `origin` must point at `repo`. Returns the tracked repo with `preferred_path` set to `path`; `HTTP 400` when the path is no checkout of `repo`.",
        )
        .query::<scm::LocalHostQuery>()
        .body::<repos::TrackBody>(),
        op(
            "repos.forget",
            Method::DELETE,
            "/api/repos/tracked/:id",
            "Forget a tracked repo, or one of its paths, as `rupu repos forget` does.\n\n`id` is the `id` from `repos.tracked`; with `path`, only that path is forgotten (the last one forgets the repo). Returns `{repo, path?, remaining}`, `remaining` the record left or null; `HTTP 404` for an untracked repo or path.",
        )
        .query::<repos::ForgetQuery>(),
        op(
            "auth.status",
            Method::GET,
            "/api/auth/status",
            "Show which provider and SCM accounts have credentials, their auth mode and SSO expiry.\n\nReturns a JSON array of `{account, kind, api_key, sso, state}`: `sso` is `{expires_at, expired, refreshable, summary}` or null; `state` ok, needs_login (an expired SSO token that can't be refreshed) or absent. Never a token; logging in stays `rupu auth login`.",
        )
        .query::<scm::LocalHostQuery>(),
    ]
}
