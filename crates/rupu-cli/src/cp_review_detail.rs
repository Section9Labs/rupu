//! Bounded read-only review projections for the owning project's inspector.
//! Account expectations are guards, never overrides. No forge mutations.
use rupu_cp::repos::{RepoListError, ReviewSelection};
use rupu_scm::{Platform, PrRef, Registry, RepoRef};
use serde_json::{json, Value};
use std::path::Path;

const MAX_BODY: usize = 16 * 1024;
const MAX_COMMENT: usize = 4 * 1024;
const MAX_DIFF: usize = 256 * 1024;
const MAX_CHECKS: usize = 40;
const MAX_THREADS: usize = 20;
const MAX_COMMENTS: usize = 5;
fn unavailable(reason: &str) -> Value {
    json!({"state":"unavailable","items":[],"truncated":false,"reason":reason})
}
fn failed() -> RepoListError {
    RepoListError::Backend("Review details unavailable".into())
}
fn text(value: &Value, max: usize) -> (String, bool) {
    let raw = value.as_str().unwrap_or_default();
    let mut end = raw.len().min(max);
    while !raw.is_char_boundary(end) {
        end -= 1;
    }
    (raw[..end].to_string(), raw.len() > end)
}
fn short(value: &Value) -> String {
    text(value, 1024).0
}
fn safe_url(value: &Value, origin_host: &str) -> Value {
    safe_link(value, origin_host, false)
}
fn safe_comment_url(value: &Value, origin_host: &str) -> Value {
    safe_link(value, origin_host, true)
}
fn safe_link(value: &Value, origin_host: &str, allow_fragment: bool) -> Value {
    let result = value
        .as_str()
        .filter(|s| s.len() <= 4096)
        .and_then(|s| reqwest::Url::parse(s).ok())
        .filter(|u| {
            matches!(u.scheme(), "http" | "https")
                && u.username().is_empty()
                && u.password().is_none()
                && u.query().is_none()
                && (allow_fragment || u.fragment().is_none())
                && u.host_str()
                    .is_some_and(|h| h.eq_ignore_ascii_case(origin_host))
        });
    json!(result.map(|u| u.to_string()))
}
fn guard_github(
    row: &Value,
    repo: &RepoRef,
    number: u32,
    selection: &ReviewSelection,
) -> Result<(), RepoListError> {
    let expected_repo = format!("{}/{}", repo.owner, repo.repo);
    if row["number"].as_u64() != Some(u64::from(number))
        || row["headRefName"].as_str() != Some(selection.branch.as_str())
        || row["headRefOid"].as_str() != Some(selection.head.as_str())
        || !row["headRepository"]["nameWithOwner"]
            .as_str()
            .is_some_and(|v| v.eq_ignore_ascii_case(&expected_repo))
    {
        return Err(RepoListError::SelectionChanged);
    }
    Ok(())
}
fn guard_gitlab(
    row: &Value,
    number: u32,
    selection: &ReviewSelection,
) -> Result<(), RepoListError> {
    if row["iid"].as_u64() != Some(u64::from(number))
        || row["source_branch"].as_str() != Some(selection.branch.as_str())
        || row["diff_refs"]["head_sha"].as_str() != Some(selection.head.as_str())
        || row["source_project_id"].as_u64().is_none()
        || row["source_project_id"] != row["target_project_id"]
    {
        return Err(RepoListError::SelectionChanged);
    }
    Ok(())
}

pub(crate) async fn read(
    registry: &Registry,
    repo: &RepoRef,
    cwd: &Path,
    origin_host: &str,
    number: u32,
    selection: &ReviewSelection,
) -> Result<Value, RepoListError> {
    let (account, detail, checks, threads, diff) = match repo.platform {
        Platform::Github => {
            let (account, extras) = registry
                .github_extras_for(repo, Some(cwd), None)
                .map_err(|_| failed())?;
            if account.to_string() != selection.account {
                return Err(RepoListError::SelectionChanged);
            }
            if !extras.matches_web_host(origin_host) {
                return Err(failed());
            }
            let value = extras
                .graphql_json(
                    REVIEW_DETAIL,
                    json!({"owner":repo.owner,"name":repo.repo,"number":number}),
                )
                .await
                .map_err(|_| failed())?;
            let row = &value["repository"]["pullRequest"];
            guard_github(row, repo, number, selection)?;
            let (body, body_truncated) = text(&row["body"], MAX_BODY);
            let detail = json!({"title":short(&row["title"]),"body":body,"body_truncated":body_truncated,
                "url":safe_url(&row["url"],origin_host),"state":short(&row["state"]).to_ascii_lowercase(),"draft":row["isDraft"].as_bool().unwrap_or(false),
                "author":short(&row["author"]["login"]),"created_at":short(&row["createdAt"]),"updated_at":short(&row["updatedAt"]),
                "base_branch":short(&row["baseRefName"]),"base_sha":short(&row["baseRefOid"])});
            let checks = github_checks(row, origin_host);
            let threads = github_threads(row, origin_host);
            let mut diff = json!({"state":"not_queried","text":"","truncated":false});
            if selection.include_diff {
                // Use the same account already selected by owning-cwd rules;
                // do not allow a caller-supplied account to select a connector.
                if let Ok((resolved, conn)) = registry.repo_for(repo, Some(cwd), Some(&account)) {
                    if resolved != account {
                        return Err(RepoListError::SelectionChanged);
                    }
                    let pr = PrRef {
                        repo: repo.clone(),
                        number,
                    };
                    match conn.diff_pr(&pr).await {
                        Ok(value) => {
                            // Diff endpoint is number-addressed. Re-read its head
                            // after retrieval; never label a moved diff as captured.
                            let after = extras
                                .graphql_json(
                                    REVIEW_IDENTITY,
                                    json!({"owner":repo.owner,"name":repo.repo,"number":number}),
                                )
                                .await
                                .map_err(|_| failed())?;
                            guard_github(
                                &after["repository"]["pullRequest"],
                                repo,
                                number,
                                selection,
                            )?;
                            if after["repository"]["pullRequest"]["baseRefOid"] != row["baseRefOid"]
                            {
                                return Err(RepoListError::SelectionChanged);
                            }
                            let (patch, truncated) = text(&json!(value.patch), MAX_DIFF);
                            diff = json!({"state":"queried","text":patch,"truncated":truncated,
                                "scope":"review_diff","atomic":false});
                        }
                        Err(_) => {
                            diff = json!({"state":"unavailable","text":"","truncated":false,"reason":"Review diff unavailable from this account"})
                        }
                    }
                } else {
                    diff = json!({"state":"unavailable","text":"","truncated":false,"reason":"Review diff connector unavailable"});
                }
            }
            (account, detail, checks, threads, diff)
        }
        Platform::Gitlab => {
            let (account, extras) = registry
                .gitlab_extras_for(repo, Some(cwd), None)
                .map_err(|_| failed())?;
            if account.to_string() != selection.account {
                return Err(RepoListError::SelectionChanged);
            }
            if !extras.matches_web_host(origin_host) {
                return Err(failed());
            }
            let raw = extras
                .review_detail(
                    repo,
                    number,
                    &selection.branch,
                    &selection.head,
                    selection.include_diff,
                )
                .await
                .map_err(|_| failed())?;
            let row = &raw["detail"];
            guard_gitlab(row, number, selection)?;
            guard_gitlab(&raw["verified"], number, selection)?;
            if raw["verified"]["diff_refs"]["start_sha"] != row["diff_refs"]["start_sha"]
                || raw["verified"]["diff_refs"]["base_sha"] != row["diff_refs"]["base_sha"]
            {
                return Err(RepoListError::SelectionChanged);
            }
            let (body, body_truncated) = text(&row["description"], MAX_BODY);
            let detail = json!({"title":short(&row["title"]),"body":body,"body_truncated":body_truncated,
                "url":safe_url(&row["web_url"],origin_host),"state":short(&row["state"]),"draft":row["draft"].as_bool().unwrap_or(false),
                "author":short(&row["author"]["username"]),"created_at":short(&row["created_at"]),"updated_at":short(&row["updated_at"]),
                "base_branch":short(&row["target_branch"]),"base_sha":short(&row["diff_refs"]["start_sha"])});
            (
                account,
                detail,
                gitlab_checks(&raw, origin_host),
                gitlab_threads(&raw, origin_host),
                gitlab_diff(&raw, selection.include_diff),
            )
        }
    };
    Ok(
        json!({"state":"queried","account":account.to_string(),"number":number,"branch":selection.branch,"head_sha":selection.head,
        "detail":detail,"checks":checks,"threads":threads,"diff":diff,"atomic":false,
        "freshness":"connector_cache_policy","scope":"origin_repository_current_branch"}),
    )
}

fn github_checks(row: &Value, host: &str) -> Value {
    let rollup = &row["commits"]["nodes"][0]["commit"]["statusCheckRollup"];
    if rollup.is_null() {
        return json!({"state":"queried","items":[],"truncated":false,"scope":"head_commit_contexts"});
    }
    let page = &rollup["contexts"];
    let Some(rows) = page["nodes"].as_array() else {
        return unavailable("Check details were not returned");
    };
    let items: Vec<_> = rows.iter().take(MAX_CHECKS).map(|r| {
        let check_run = r["__typename"] == "CheckRun";
        let state = if check_run { r["conclusion"].as_str().or(r["status"].as_str()) } else { r["state"].as_str() };
        json!({"name":short(if check_run { &r["name"] } else { &r["context"] }),
            "state":state.map(str::to_ascii_lowercase),"url":safe_url(if check_run { &r["detailsUrl"] } else { &r["targetUrl"] },host)})
    }).collect();
    json!({"state":"queried","items":items,"truncated":rows.len()>MAX_CHECKS || page["pageInfo"]["hasNextPage"]==true,"scope":"head_commit_contexts"})
}
fn github_threads(row: &Value, host: &str) -> Value {
    let page = &row["reviewThreads"];
    let Some(rows) = page["nodes"].as_array() else {
        return unavailable("Inline review conversations were not returned");
    };
    let items: Vec<_> = rows.iter().take(MAX_THREADS).map(|r| {
        let comments = r["comments"]["nodes"].as_array();
        let notes: Vec<_> = comments.into_iter().flatten().take(MAX_COMMENTS).map(|c| {
            let (body,truncated) = text(&c["body"],MAX_COMMENT);
            json!({"id":short(&c["id"]),"author":short(&c["author"]["login"]),"body":body,"body_truncated":truncated,
                "created_at":short(&c["createdAt"]),"url":safe_comment_url(&c["url"],host)})
        }).collect();
        json!({"id":short(&r["id"]),"path":short(&r["path"]),"line":r["line"],"resolved":r["isResolved"],"comments":notes,
            "comments_truncated":comments.is_some_and(|c|c.len()>MAX_COMMENTS) || r["comments"]["pageInfo"]["hasNextPage"]==true})
    }).collect();
    json!({"state":"queried","items":items,"truncated":rows.len()>MAX_THREADS || page["pageInfo"]["hasNextPage"]==true,"scope":"inline_review_threads"})
}
fn gitlab_checks(raw: &Value, host: &str) -> Value {
    if raw["no_pipeline"] == true {
        return json!({"state":"queried","items":[],"truncated":false,"scope":"head_pipeline_jobs"});
    }
    let Some(rows) = raw["jobs"].as_array() else {
        return unavailable("Head pipeline jobs unavailable from this account");
    };
    let items: Vec<_> = rows.iter().take(MAX_CHECKS).map(|r|json!({"name":short(&r["name"]),"state":short(&r["status"]),"url":safe_url(&r["web_url"],host)})).collect();
    json!({"state":"queried","items":items,"truncated":rows.len()>MAX_CHECKS,"scope":"head_pipeline_jobs"})
}
fn gitlab_threads(raw: &Value, host: &str) -> Value {
    let Some(rows) = raw["discussions"].as_array() else {
        return unavailable("Merge request discussions unavailable from this account");
    };
    let items: Vec<_> = rows.iter().take(MAX_THREADS).map(|r| {
        let comments = r["notes"].as_array();
        let notes: Vec<_> = comments.into_iter().flatten().take(MAX_COMMENTS).map(|c| {
            let (body,truncated)=text(&c["body"],MAX_COMMENT);
            json!({"id":c["id"].as_u64().map(|n|n.to_string()),"author":short(&c["author"]["username"]),"body":body,"body_truncated":truncated,
                "created_at":short(&c["created_at"]),"url":safe_comment_url(&c["url"],host)})
        }).collect();
        let first = &r["notes"][0];
        json!({"id":short(&r["id"]),"path":short(&first["position"]["new_path"]),"line":first["position"]["new_line"],
            "resolved":first["resolved"],"comments":notes,"comments_truncated":comments.is_some_and(|c|c.len()>MAX_COMMENTS)})
    }).collect();
    json!({"state":"queried","items":items,"truncated":rows.len()>MAX_THREADS,"scope":"merge_request_discussions"})
}
fn gitlab_diff(raw: &Value, include: bool) -> Value {
    if !include {
        return json!({"state":"not_queried","text":"","truncated":false});
    }
    let Some(rows) = raw["diffs"].as_array() else {
        return json!({"state":"unavailable","text":"","truncated":false,"reason":"Merge request diff unavailable from this account"});
    };
    let mut patch = String::new();
    let mut truncated = rows.len() > 40;
    for row in rows.iter().take(40) {
        let old = short(&row["old_path"]);
        let new = short(&row["new_path"]);
        let (body, cut) = text(&row["diff"], MAX_DIFF.saturating_sub(patch.len()));
        // Header names are data, never filesystem paths or commands.
        let block = format!("diff --git a/{old} b/{new}\n{body}\n");
        let (bounded, cut_block) = text(&json!(block), MAX_DIFF.saturating_sub(patch.len()));
        patch.push_str(&bounded);
        truncated |= cut || cut_block || row["collapsed"] == true || row["too_large"] == true;
        if patch.len() >= MAX_DIFF {
            truncated = true;
            break;
        }
    }
    json!({"state":"queried","text":patch,"truncated":truncated,"scope":"review_diff","atomic":false})
}

const REVIEW_IDENTITY: &str = r#"query RupuReviewIdentity($owner:String!,$name:String!,$number:Int!){repository(owner:$owner,name:$name){pullRequest(number:$number){number headRefName headRefOid baseRefOid headRepository{nameWithOwner}}}}"#;
const REVIEW_DETAIL: &str = r#"query RupuReviewDetail($owner:String!,$name:String!,$number:Int!){repository(owner:$owner,name:$name){pullRequest(number:$number){
 number title body url state isDraft author{login} createdAt updatedAt headRefName headRefOid baseRefName baseRefOid headRepository{nameWithOwner}
 commits(last:1){nodes{commit{statusCheckRollup{contexts(first:41){pageInfo{hasNextPage} nodes{__typename ... on CheckRun{name status conclusion detailsUrl} ... on StatusContext{context state targetUrl}}}}}}}
 reviewThreads(first:21){pageInfo{hasNextPage} nodes{id path line isResolved comments(first:6){pageInfo{hasNextPage} nodes{id author{login} body createdAt url}}}}
}}}"#;

#[cfg(test)]
mod tests {
    use super::*;
    use rupu_scm::AccountId;
    use std::sync::Arc;
    fn selection() -> ReviewSelection {
        ReviewSelection {
            account: "fixture".into(),
            branch: "feature/a".into(),
            head: "abc".into(),
            include_diff: false,
        }
    }
    fn repo(platform: Platform) -> RepoRef {
        RepoRef {
            platform,
            owner: "o".into(),
            repo: "r".into(),
        }
    }
    fn github_row() -> Value {
        json!({"number":9,"title":"Review","body":"Description","state":"OPEN","isDraft":false,
        "url":"https://github.com/o/r/pull/9","headRefName":"feature/a","headRefOid":"abc","baseRefName":"main","baseRefOid":"def",
        "headRepository":{"nameWithOwner":"o/r"},"author":{"login":"author"},
        "commits":{"nodes":[{"commit":{"statusCheckRollup":{"contexts":{"pageInfo":{"hasNextPage":false},"nodes":[
            {"__typename":"CheckRun","name":"tests","status":"COMPLETED","conclusion":"FAILURE"},
            {"__typename":"StatusContext","context":"build","state":"SUCCESS"}]}}}}]},
        "reviewThreads":{"pageInfo":{"hasNextPage":false},"nodes":[{"id":"thread1","path":"src/main.rs","line":4,"isResolved":false,
            "comments":{"pageInfo":{"hasNextPage":false},"nodes":[{"id":"c1","author":{"login":"reviewer"},"body":"Please cover the edge case","createdAt":"2026-11-08T00:00:00Z"}]}}]}})
    }

    #[test]
    fn projections_preserve_failure_and_bound_nested_text_without_splitting_unicode() {
        let mut row = github_row();
        row["reviewThreads"]["nodes"][0]["comments"]["nodes"][0]["body"] =
            json!("é".repeat(MAX_COMMENT));
        let checks = github_checks(&row, "github.com");
        assert_eq!(checks["items"][0]["state"], "failure");
        assert_eq!(checks["items"][1]["state"], "success");
        let threads = github_threads(&row, "github.com");
        let comment = &threads["items"][0]["comments"][0];
        assert_eq!(comment["body"].as_str().unwrap().len(), MAX_COMMENT);
        assert_eq!(comment["body_truncated"], true);
        assert_eq!(threads["scope"], "inline_review_threads");
        assert!(safe_url(&json!("https://user:secret@github.com/o/r"), "github.com").is_null());
        assert!(safe_url(&json!("https://other.example/o/r"), "github.com").is_null());
    }

    #[test]
    fn refuses_forks_changed_branch_head_or_number() {
        let mut row = github_row();
        assert!(guard_github(&row, &repo(Platform::Github), 9, &selection()).is_ok());
        for (field, value) in [
            ("number", json!(10)),
            ("headRefName", json!("other")),
            ("headRefOid", json!("new")),
            ("headRepository", json!({"nameWithOwner":"foreign/r"})),
        ] {
            let old = row[field].clone();
            row[field] = value;
            assert!(matches!(
                guard_github(&row, &repo(Platform::Github), 9, &selection()),
                Err(RepoListError::SelectionChanged)
            ));
            row[field] = old;
        }
    }

    #[tokio::test]
    async fn github_exact_account_and_host_guard_precede_network_and_diff_is_lazy() {
        use httpmock::{Method::POST, MockServer};
        use rupu_scm::connectors::github::{GithubClient, GithubExtras};
        rupu_scm::install_default_crypto_provider();
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(POST)
                .path("/api/graphql")
                .header("authorization", "Bearer fixture-only")
                .json_body(
                    json!({"query":REVIEW_DETAIL,"variables":{"owner":"o","name":"r","number":9}}),
                );
            then.status(200)
                .json_body(json!({"data":{"repository":{"pullRequest":github_row()}}}));
        });
        let mut registry = Registry::empty();
        registry.insert_github_extras_account(
            AccountId::new("fixture"),
            Arc::new(GithubExtras::new(GithubClient::new(
                "fixture-only".into(),
                Some(server.base_url()),
                Some(2),
                Arc::new(rupu_netflow::NullSink),
            ))),
        );
        let mut selected = selection();
        selected.account = "other".into();
        assert!(matches!(
            read(
                &registry,
                &repo(Platform::Github),
                Path::new("/fixture"),
                "127.0.0.1",
                9,
                &selected
            )
            .await,
            Err(RepoListError::SelectionChanged)
        ));
        assert!(read(
            &registry,
            &repo(Platform::Github),
            Path::new("/fixture"),
            "github.com",
            9,
            &selection()
        )
        .await
        .is_err());
        mock.assert_hits(0);
        let value = read(
            &registry,
            &repo(Platform::Github),
            Path::new("/fixture"),
            "127.0.0.1",
            9,
            &selection(),
        )
        .await
        .unwrap();
        mock.assert_hits(1);
        assert_eq!(value["number"], 9);
        assert_eq!(value["account"], "fixture");
        assert_eq!(value["diff"]["state"], "not_queried");
        assert_eq!(value["checks"]["items"][0]["state"], "failure");
        assert_eq!(
            value["threads"]["items"][0]["comments"][0]["author"],
            "reviewer"
        );
    }

    #[tokio::test]
    async fn gitlab_gets_bounded_first_pages_and_preserves_partial_failure() {
        use httpmock::{Method::GET, MockServer};
        use rupu_scm::connectors::gitlab::{GitlabClient, GitlabExtras};
        rupu_scm::install_default_crypto_provider();
        let server = MockServer::start();
        let detail=server.mock(|when,then| {
            when.method(GET).path("/projects/o%2Fr/merge_requests/9");
            then.status(200).json_body(json!({"iid":9,"title":"MR","source_branch":"feature/a","target_branch":"main","description":"Body",
                "source_project_id":2,"target_project_id":2,"diff_refs":{"head_sha":"abc","start_sha":"def"},"head_pipeline":{"id":5,"status":"failed","sha":"abc"}}));
        });
        let jobs = server.mock(|when, then| {
            when.method(GET)
                .path("/projects/o%2Fr/pipelines/5/jobs")
                .query_param("per_page", "41")
                .query_param("page", "1");
            then.status(200)
                .json_body(json!([{"name":"unit","status":"failed"}]));
        });
        let discussions = server.mock(|when, then| {
            when.method(GET)
                .path("/projects/o%2Fr/merge_requests/9/discussions")
                .query_param("per_page", "21");
            then.status(403);
        });
        let diffs=server.mock(|when,then| { when.method(GET).path("/projects/o%2Fr/merge_requests/9/diffs").query_param("per_page","41");
            then.status(200).json_body(json!([{"old_path":"a.rs","new_path":"a.rs","diff":"@@ -1 +1 @@\n-a\n+b","too_large":true}])); });
        let mut registry = Registry::empty();
        registry.insert_gitlab_extras_account(
            AccountId::new("fixture"),
            Arc::new(GitlabExtras::new(GitlabClient::new(
                "fixture-only".into(),
                Some(server.base_url()),
                Some(2),
                Arc::new(rupu_netflow::NullSink),
            ))),
        );
        let mut selected = selection();
        selected.include_diff = true;
        let value = read(
            &registry,
            &repo(Platform::Gitlab),
            Path::new("/fixture"),
            "127.0.0.1",
            9,
            &selected,
        )
        .await
        .unwrap();
        detail.assert_hits(2);
        jobs.assert_hits(1);
        discussions.assert_hits(1);
        diffs.assert_hits(1);
        assert_eq!(value["checks"]["items"][0]["state"], "failed");
        assert_eq!(value["threads"]["state"], "unavailable");
        assert_eq!(value["diff"]["truncated"], true);
    }

    #[test]
    fn provider_pages_and_diff_are_never_reported_complete_when_cut() {
        let mut row = github_row();
        row["reviewThreads"]["pageInfo"]["hasNextPage"] = json!(true);
        row["reviewThreads"]["nodes"][0]["comments"]["pageInfo"]["hasNextPage"] = json!(true);
        assert_eq!(github_threads(&row, "github.com")["truncated"], true);
        assert_eq!(
            github_threads(&row, "github.com")["items"][0]["comments_truncated"],
            true
        );
        let raw = json!({"diffs":[{"old_path":"a","new_path":"a","diff":"x".repeat(MAX_DIFF+1)}]});
        let diff = gitlab_diff(&raw, true);
        assert!(diff["text"].as_str().unwrap().len() <= MAX_DIFF);
        assert_eq!(diff["truncated"], true);
    }
}
