//! Arc 2 Task 5: proves `rupu-mcp` tool dispatch actually routes to the
//! account the rule engine selects, not merely that the call succeeds.
//!
//! Each fake `RepoConnector` below is tagged with a distinct
//! `default_branch` string per account, so a wrong-account bug (e.g. a
//! dispatch fn that still discards the resolved `RepoRef`/`account` and
//! falls back to `Registry::repo(platform)`'s lexicographically-first
//! pick) fails the assertion on the returned *content*, not just the
//! call's exit status. An assertion that only checked `is_ok()` would
//! pass even with the pre-migration shim wired back in.

use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use rupu_mcp::{McpPermission, ToolDispatcher};
use rupu_scm::rules::Rule;
use rupu_scm::{
    AccountId, Branch, Comment, CreateIssue, CreatePr, Diff, FileContent, Issue, IssueConnector,
    IssueFilter, IssueRef, IssueState, IssueTracker, Platform, Pr, PrFilter, PrRef, Registry, Repo,
    RepoConnector, RepoRef, ScmError,
};

/// A `RepoConnector` whose `get_repo`/`list_repos` responses are tagged
/// with `branch_tag` — the one distinguishing fact the tests assert on.
struct TaggedRepoConnector {
    branch_tag: &'static str,
}

#[async_trait]
impl RepoConnector for TaggedRepoConnector {
    fn platform(&self) -> Platform {
        Platform::Github
    }
    async fn list_repos(&self) -> Result<Vec<Repo>, ScmError> {
        Ok(vec![Repo {
            r: RepoRef {
                platform: Platform::Github,
                owner: "acme".into(),
                repo: "api".into(),
            },
            default_branch: self.branch_tag.into(),
            clone_url_https: String::new(),
            clone_url_ssh: String::new(),
            private: false,
            description: None,
        }])
    }
    async fn get_repo(&self, r: &RepoRef) -> Result<Repo, ScmError> {
        Ok(Repo {
            r: r.clone(),
            default_branch: self.branch_tag.into(),
            clone_url_https: String::new(),
            clone_url_ssh: String::new(),
            private: false,
            description: None,
        })
    }
    async fn list_branches(&self, _r: &RepoRef) -> Result<Vec<Branch>, ScmError> {
        unimplemented!()
    }
    async fn create_branch(
        &self,
        _r: &RepoRef,
        _name: &str,
        _from_sha: &str,
    ) -> Result<Branch, ScmError> {
        unimplemented!()
    }
    async fn read_file(
        &self,
        _r: &RepoRef,
        _path: &str,
        _ref_: Option<&str>,
    ) -> Result<FileContent, ScmError> {
        unimplemented!()
    }
    async fn list_prs(&self, _r: &RepoRef, _filter: PrFilter) -> Result<Vec<Pr>, ScmError> {
        unimplemented!()
    }
    async fn get_pr(&self, _p: &PrRef) -> Result<Pr, ScmError> {
        unimplemented!()
    }
    async fn diff_pr(&self, _p: &PrRef) -> Result<Diff, ScmError> {
        unimplemented!()
    }
    async fn comment_pr(&self, _p: &PrRef, _body: &str) -> Result<Comment, ScmError> {
        unimplemented!()
    }
    async fn create_pr(&self, _r: &RepoRef, _opts: CreatePr) -> Result<Pr, ScmError> {
        unimplemented!()
    }
    async fn clone_to(&self, _r: &RepoRef, _dir: &Path) -> Result<(), ScmError> {
        unimplemented!()
    }
}

/// A `RepoConnector` whose `list_repos` always fails — used to prove
/// `scm.repos.list`'s fan-out is warn-and-continue, not all-or-nothing
/// (Task 5 review finding 1).
struct FailingRepoConnector;

#[async_trait]
impl RepoConnector for FailingRepoConnector {
    fn platform(&self) -> Platform {
        Platform::Github
    }
    async fn list_repos(&self) -> Result<Vec<Repo>, ScmError> {
        Err(ScmError::Unauthorized {
            platform: "github".into(),
            hint: "token expired".into(),
        })
    }
    async fn get_repo(&self, _r: &RepoRef) -> Result<Repo, ScmError> {
        unimplemented!()
    }
    async fn list_branches(&self, _r: &RepoRef) -> Result<Vec<Branch>, ScmError> {
        unimplemented!()
    }
    async fn create_branch(
        &self,
        _r: &RepoRef,
        _name: &str,
        _from_sha: &str,
    ) -> Result<Branch, ScmError> {
        unimplemented!()
    }
    async fn read_file(
        &self,
        _r: &RepoRef,
        _path: &str,
        _ref_: Option<&str>,
    ) -> Result<FileContent, ScmError> {
        unimplemented!()
    }
    async fn list_prs(&self, _r: &RepoRef, _filter: PrFilter) -> Result<Vec<Pr>, ScmError> {
        unimplemented!()
    }
    async fn get_pr(&self, _p: &PrRef) -> Result<Pr, ScmError> {
        unimplemented!()
    }
    async fn diff_pr(&self, _p: &PrRef) -> Result<Diff, ScmError> {
        unimplemented!()
    }
    async fn comment_pr(&self, _p: &PrRef, _body: &str) -> Result<Comment, ScmError> {
        unimplemented!()
    }
    async fn create_pr(&self, _r: &RepoRef, _opts: CreatePr) -> Result<Pr, ScmError> {
        unimplemented!()
    }
    async fn clone_to(&self, _r: &RepoRef, _dir: &Path) -> Result<(), ScmError> {
        unimplemented!()
    }
}

/// A `RepoConnector` tagged with a specific `Platform`, distinct from
/// `TaggedRepoConnector`'s hardcoded GitHub — used to prove
/// `scm.repos.list`'s `account` argument is checked against the
/// requested `platform` rather than trusted blindly (Task 5 review
/// finding 2).
struct PlatformTaggedRepoConnector {
    platform: Platform,
}

#[async_trait]
impl RepoConnector for PlatformTaggedRepoConnector {
    fn platform(&self) -> Platform {
        self.platform
    }
    async fn list_repos(&self) -> Result<Vec<Repo>, ScmError> {
        Ok(vec![])
    }
    async fn get_repo(&self, _r: &RepoRef) -> Result<Repo, ScmError> {
        unimplemented!()
    }
    async fn list_branches(&self, _r: &RepoRef) -> Result<Vec<Branch>, ScmError> {
        unimplemented!()
    }
    async fn create_branch(
        &self,
        _r: &RepoRef,
        _name: &str,
        _from_sha: &str,
    ) -> Result<Branch, ScmError> {
        unimplemented!()
    }
    async fn read_file(
        &self,
        _r: &RepoRef,
        _path: &str,
        _ref_: Option<&str>,
    ) -> Result<FileContent, ScmError> {
        unimplemented!()
    }
    async fn list_prs(&self, _r: &RepoRef, _filter: PrFilter) -> Result<Vec<Pr>, ScmError> {
        unimplemented!()
    }
    async fn get_pr(&self, _p: &PrRef) -> Result<Pr, ScmError> {
        unimplemented!()
    }
    async fn diff_pr(&self, _p: &PrRef) -> Result<Diff, ScmError> {
        unimplemented!()
    }
    async fn comment_pr(&self, _p: &PrRef, _body: &str) -> Result<Comment, ScmError> {
        unimplemented!()
    }
    async fn create_pr(&self, _r: &RepoRef, _opts: CreatePr) -> Result<Pr, ScmError> {
        unimplemented!()
    }
    async fn clone_to(&self, _r: &RepoRef, _dir: &Path) -> Result<(), ScmError> {
        unimplemented!()
    }
}

/// An `IssueConnector` whose `list_issues` response count is tagged per
/// account — the issues-side counterpart of `TaggedRepoConnector`.
struct CountingIssueConnector {
    open_count: usize,
}

#[async_trait]
impl IssueConnector for CountingIssueConnector {
    fn tracker(&self) -> IssueTracker {
        IssueTracker::Github
    }
    async fn list_issues(
        &self,
        _project: &str,
        _filter: IssueFilter,
    ) -> Result<Vec<Issue>, ScmError> {
        let now = chrono::Utc::now();
        Ok((0..self.open_count)
            .map(|n| Issue {
                r: IssueRef {
                    tracker: IssueTracker::Github,
                    project: "acme/api".into(),
                    number: n as u64,
                },
                title: format!("issue {n}"),
                body: String::new(),
                state: IssueState::Open,
                labels: vec![],
                label_colors: Default::default(),
                author: "someone".into(),
                created_at: now,
                updated_at: now,
            })
            .collect())
    }
    async fn get_issue(&self, _i: &IssueRef) -> Result<Issue, ScmError> {
        unimplemented!()
    }
    async fn comment_issue(&self, _i: &IssueRef, _body: &str) -> Result<Comment, ScmError> {
        unimplemented!()
    }
    async fn create_issue(&self, _project: &str, _opts: CreateIssue) -> Result<Issue, ScmError> {
        unimplemented!()
    }
    async fn update_issue_state(&self, _i: &IssueRef, _state: IssueState) -> Result<(), ScmError> {
        unimplemented!()
    }
}

fn two_github_accounts_repo_registry() -> Registry {
    let mut reg = Registry::empty();
    reg.insert_repo_account(
        AccountId::new("gh-work"),
        Platform::Github,
        Arc::new(TaggedRepoConnector {
            branch_tag: "work-default-branch",
        }),
    );
    reg.insert_repo_account(
        AccountId::new("gh-personal"),
        Platform::Github,
        Arc::new(TaggedRepoConnector {
            branch_tag: "personal-default-branch",
        }),
    );
    reg
}

#[tokio::test]
async fn owner_rule_routes_scm_repos_get_to_the_matching_account() {
    let mut reg = two_github_accounts_repo_registry();
    reg.set_rules(vec![Rule {
        owner: Some("acme/*".into()),
        path: None,
        account: AccountId::new("gh-work"),
    }]);
    let dispatcher = ToolDispatcher::new(Arc::new(reg), McpPermission::allow_all());

    let result = dispatcher
        .call(
            "scm.repos.get",
            serde_json::json!({"platform": "github", "owner": "acme", "repo": "api"}),
        )
        .await
        .expect("dispatch should succeed — one account matches the owner rule");

    let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
    assert_eq!(
        parsed["default_branch"], "work-default-branch",
        "the owner rule names gh-work; a dispatch that still discards the resolved RepoRef \
         (falling back to Registry::repo(platform)'s lexicographically-first account) would \
         return gh-personal's tag instead — response was: {result}"
    );
}

#[tokio::test]
async fn explicit_account_argument_overrides_the_owner_rule() {
    let mut reg = two_github_accounts_repo_registry();
    // The rule points at gh-personal — which is also the account
    // `Registry::repo(platform)`'s shim would pick anyway (bare-name
    // preference finds nothing, so it falls back to the
    // lexicographically-first GitHub account, and "gh-personal" sorts
    // before "gh-work"). Deliberately: if this test instead asked for
    // gh-personal explicitly, a leftover shim call that silently ignores
    // `account` and always returns its own lexicographic pick would
    // satisfy the assertion by coincidence. Asking for gh-work here — the
    // account that is neither the rule's target nor the shim's arbitrary
    // pick — means only a dispatch that actually honors the explicit
    // tier can produce "work-default-branch".
    reg.set_rules(vec![Rule {
        owner: Some("acme/*".into()),
        path: None,
        account: AccountId::new("gh-personal"),
    }]);
    let dispatcher = ToolDispatcher::new(Arc::new(reg), McpPermission::allow_all());

    let result = dispatcher
        .call(
            "scm.repos.get",
            serde_json::json!({
                "platform": "github",
                "owner": "acme",
                "repo": "api",
                "account": "gh-work",
            }),
        )
        .await
        .expect("dispatch should succeed with an explicit, valid account");

    let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
    assert_eq!(
        parsed["default_branch"], "work-default-branch",
        "an explicit `account` argument must override the owner rule — response was: {result}"
    );
}

#[tokio::test]
async fn unknown_explicit_account_errors_rather_than_falling_back() {
    let reg = two_github_accounts_repo_registry();
    let dispatcher = ToolDispatcher::new(Arc::new(reg), McpPermission::allow_all());

    let err = dispatcher
        .call(
            "scm.repos.get",
            serde_json::json!({
                "platform": "github",
                "owner": "acme",
                "repo": "api",
                "account": "gh-typo",
            }),
        )
        .await
        .expect_err("a typo'd account must fail, not silently resolve to some other account");

    let msg = err.to_string();
    assert!(
        msg.contains("gh-typo"),
        "error should name the bad account, got: {msg}"
    );
}

#[tokio::test]
async fn scm_repos_list_fans_out_and_tags_each_row_with_its_account() {
    let reg = two_github_accounts_repo_registry();
    let dispatcher = ToolDispatcher::new(Arc::new(reg), McpPermission::allow_all());

    let result = dispatcher
        .call("scm.repos.list", serde_json::json!({"platform": "github"}))
        .await
        .expect("fan-out list should succeed across both accounts");

    let parsed: Vec<serde_json::Value> = serde_json::from_str(&result).unwrap();
    assert_eq!(
        parsed.len(),
        2,
        "expected one row per configured GitHub account, got: {result}"
    );
    let tags: std::collections::BTreeSet<String> = parsed
        .iter()
        .map(|row| {
            format!(
                "{}={}",
                row["account"].as_str().unwrap(),
                row["default_branch"].as_str().unwrap()
            )
        })
        .collect();
    assert_eq!(
        tags,
        std::collections::BTreeSet::from([
            "gh-personal=personal-default-branch".to_string(),
            "gh-work=work-default-branch".to_string(),
        ]),
        "each row's `account` field must match the connector that actually produced it"
    );
}

#[tokio::test]
async fn two_github_accounts_route_issues_list_by_owner_rule() {
    let mut reg = Registry::empty();
    reg.insert_issue_account(
        AccountId::new("gh-work"),
        Platform::Github,
        Arc::new(CountingIssueConnector { open_count: 7 }),
    );
    reg.insert_issue_account(
        AccountId::new("gh-personal"),
        Platform::Github,
        Arc::new(CountingIssueConnector { open_count: 3 }),
    );
    // The rule targets gh-work (7 issues) deliberately, not gh-personal:
    // `Registry::issues(tracker)`'s shim falls back to the
    // lexicographically-first GitHub account when there's no bare
    // "github" account, and "gh-personal" sorts first. A dispatch that
    // still called the shim would return 3 (gh-personal's count) by
    // coincidence if the rule pointed at gh-personal — pointing it at
    // gh-work instead means only the real rule engine can produce 7.
    reg.set_rules(vec![Rule {
        owner: Some("acme/*".into()),
        path: None,
        account: AccountId::new("gh-work"),
    }]);
    let dispatcher = ToolDispatcher::new(Arc::new(reg), McpPermission::allow_all());

    let result = dispatcher
        .call(
            "issues.list",
            serde_json::json!({"tracker": "github", "project": "acme/api"}),
        )
        .await
        .expect("dispatch should succeed — the project string recovers a RepoRef the owner rule can match");

    let parsed: Vec<serde_json::Value> = serde_json::from_str(&result).unwrap();
    assert_eq!(
        parsed.len(),
        7,
        "the owner rule names gh-work (7 issues); a dispatch that still ignores the \
         recovered RepoRef would route to whichever account Registry::issues(tracker)'s shim \
         picks instead (gh-personal, 3 issues) — response had {} issues: {result}",
        parsed.len()
    );
}

// ── Task 5 review round 1 — findings 1, 2, 3, 5 ────────────────────────

#[tokio::test]
async fn scm_repos_list_skips_a_failing_account_and_still_returns_the_others() {
    // Review finding 1: the fan-out must be warn-and-continue, not
    // all-or-nothing. One account whose `list_repos` fails (expired
    // token, etc.) must not abort the whole tool call and lose every
    // OTHER account's repos.
    let mut reg = Registry::empty();
    reg.insert_repo_account(
        AccountId::new("gh-broken"),
        Platform::Github,
        Arc::new(FailingRepoConnector),
    );
    reg.insert_repo_account(
        AccountId::new("gh-work"),
        Platform::Github,
        Arc::new(TaggedRepoConnector {
            branch_tag: "work-default-branch",
        }),
    );
    let dispatcher = ToolDispatcher::new(Arc::new(reg), McpPermission::allow_all());

    let result = dispatcher
        .call("scm.repos.list", serde_json::json!({"platform": "github"}))
        .await
        .expect(
            "one failing account among several must not fail the whole call — \
             a pre-fix all-or-nothing fan-out would propagate FailingRepoConnector's error here",
        );

    let parsed: Vec<serde_json::Value> = serde_json::from_str(&result).unwrap();
    assert_eq!(
        parsed.len(),
        1,
        "expected exactly gh-work's one repo, with gh-broken skipped — got: {result}"
    );
    assert_eq!(parsed[0]["account"], "gh-work");
    assert_eq!(parsed[0]["default_branch"], "work-default-branch");
}

#[tokio::test]
async fn scm_repos_list_errors_when_every_account_fails() {
    // The other half of finding 1: warn-and-continue must not paper over
    // a total outage by silently returning an empty list.
    let mut reg = Registry::empty();
    reg.insert_repo_account(
        AccountId::new("gh-broken-1"),
        Platform::Github,
        Arc::new(FailingRepoConnector),
    );
    reg.insert_repo_account(
        AccountId::new("gh-broken-2"),
        Platform::Github,
        Arc::new(FailingRepoConnector),
    );
    let dispatcher = ToolDispatcher::new(Arc::new(reg), McpPermission::allow_all());

    let err = dispatcher
        .call("scm.repos.list", serde_json::json!({"platform": "github"}))
        .await
        .expect_err("every account failing must surface as an error, not an empty list");

    let msg = err.to_string();
    assert!(
        msg.contains("gh-broken-1") && msg.contains("gh-broken-2"),
        "error should name every account that failed, got: {msg}"
    );
}

#[tokio::test]
async fn scm_repos_list_rejects_an_account_of_the_wrong_platform() {
    // Review finding 2: `repo_by_account` does no kind filtering, so an
    // explicit `account` naming a GitLab account under a
    // `{"platform":"github"}` request must be rejected rather than
    // silently listing GitLab repos.
    let mut reg = Registry::empty();
    reg.insert_repo_account(
        AccountId::new("gl-personal"),
        Platform::Gitlab,
        Arc::new(PlatformTaggedRepoConnector {
            platform: Platform::Gitlab,
        }),
    );
    let dispatcher = ToolDispatcher::new(Arc::new(reg), McpPermission::allow_all());

    let err = dispatcher
        .call(
            "scm.repos.list",
            serde_json::json!({"platform": "github", "account": "gl-personal"}),
        )
        .await
        .expect_err("a GitLab account under a github platform request must be rejected");

    let msg = err.to_string();
    assert!(
        msg.contains("gl-personal") && msg.contains("gitlab") && msg.contains("github"),
        "error should name the account and both the actual and requested platform, got: {msg}"
    );
}

#[tokio::test]
async fn scm_repos_list_account_alone_resolves_without_a_platform_argument() {
    // Companion to finding 2: `account` alone must fully determine the
    // connector — the platform check must not force a `[scm.default]`
    // lookup (and thus an error) when the caller only gave `account`.
    let mut reg = Registry::empty();
    reg.insert_repo_account(
        AccountId::new("gl-personal"),
        Platform::Gitlab,
        Arc::new(PlatformTaggedRepoConnector {
            platform: Platform::Gitlab,
        }),
    );
    let dispatcher = ToolDispatcher::new(Arc::new(reg), McpPermission::allow_all());

    dispatcher
        .call(
            "scm.repos.list",
            serde_json::json!({"account": "gl-personal"}),
        )
        .await
        .expect(
            "an explicit account with no platform argument must resolve via the account alone, \
             not fail with \"no platform arg and no [scm.default] configured\"",
        );
}

#[tokio::test]
async fn scm_repos_list_unknown_account_names_the_configured_candidates() {
    // Review finding 3: an unknown `account` on the list path must use
    // the real `AccountError::UnknownAccount` (which names the
    // configured candidates an agent could retry with), not a bare
    // `InvalidArgs("no such account: ...")` with no candidate list.
    let reg = two_github_accounts_repo_registry();
    let dispatcher = ToolDispatcher::new(Arc::new(reg), McpPermission::allow_all());

    let err = dispatcher
        .call(
            "scm.repos.list",
            serde_json::json!({"platform": "github", "account": "gh-typo"}),
        )
        .await
        .expect_err("a typo'd account must fail, not silently resolve to some other account");

    let msg = err.to_string();
    assert!(
        msg.contains("gh-typo"),
        "error should name the bad account, got: {msg}"
    );
    assert!(
        msg.contains("gh-work") && msg.contains("gh-personal"),
        "error should name the configured candidates so an agent can self-correct, got: {msg}"
    );
}

#[tokio::test]
async fn single_account_and_no_rules_still_resolves_without_an_account_argument() {
    // Review finding 5: back-compat through the SoleAccount tier is the
    // arc's named load-bearing constraint. Unit-tested at the rules.rs
    // level already, but nothing at the dispatch level guarded it — one
    // account, no `[[scm.rules]]`, no `account` argument must keep
    // working exactly as it did before Arc 2.
    let mut reg = Registry::empty();
    reg.insert_repo_account(
        AccountId::new("github"),
        Platform::Github,
        Arc::new(TaggedRepoConnector {
            branch_tag: "only-account-branch",
        }),
    );
    let dispatcher = ToolDispatcher::new(Arc::new(reg), McpPermission::allow_all());

    let result = dispatcher
        .call(
            "scm.repos.get",
            serde_json::json!({"platform": "github", "owner": "acme", "repo": "api"}),
        )
        .await
        .expect("a single configured account with no rules must resolve with no account argument");

    let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
    assert_eq!(parsed["default_branch"], "only-account-branch");
}
