use super::providers::{parse_github, parse_gitlab, parse_jira, parse_linear};
use super::*;
use serde_json::json;

fn request(project: &str) -> DiscoveryRequest {
    DiscoveryRequest {
        project: project.into(),
        state: DiscoveryState::All,
        title_query: None,
        after: None,
    }
}
fn github_row() -> Value {
    json!({"number":42,"title":"A current issue","state":"open","updated_at":"2026-11-09T10:00:00Z","html_url":"https://github.example/org/repo/issues/42","repository_url":"https://api.github.example/repos/org/repo","url":"https://api.github.example/repos/org/repo/issues/42"})
}
fn gitlab_row() -> Value {
    json!({"iid":42,"project_id":7,"title":"A current issue","state":"opened","updated_at":"2026-11-09T10:00:00Z","web_url":"https://gitlab.example/group/repo/-/issues/42"})
}
fn jira_page() -> Value {
    json!({"issues":[{"id":"1234","key":"ENG-42","self":"https://jira.example/rest/api/3/issue/1234","fields":{"summary":"A current issue","project":{"id":"7","key":"ENG"},"updated":"2026-11-09T10:00:00.000+0000","status":{"statusCategory":{"key":"indeterminate"}}}}],"isLast":true})
}
fn linear_page() -> Value {
    json!({"team":{"id":"team-id","key":"ENG","issues":{"nodes":[{"identifier":"ENG-42","title":"A current issue","updatedAt":"2026-11-09T10:00:00Z","url":"https://linear.app/team/issue/ENG-42/title","team":{"id":"team-id","key":"ENG"},"state":{"type":"started"}}],"pageInfo":{"hasNextPage":false,"endCursor":"cursor"}}}})
}

#[test]
fn discovery_github_rows_require_both_returned_repository_and_issue_identity() {
    let r = request("org/repo");
    let origin = "https://api.github.example";
    let (rows, next, end, truncated) = parse_github(&json!([github_row()]), &r, origin, 1).unwrap();
    assert_eq!(rows[0].number, "42");
    assert_eq!(rows[0].state, "open");
    assert!(next.is_none() && end && !truncated);
    for (field, value) in [
        (
            "repository_url",
            "https://api.github.example/repos/other/repo",
        ),
        ("url", "https://api.github.example/repos/org/repo/issues/43"),
        ("number", "042"),
    ] {
        let mut row = github_row();
        row[field] = json!(value);
        assert!(parse_github(&json!([row]), &r, origin, 1).is_err());
    }
    let mut row = github_row();
    row["pull_request"] = json!({"url":"ignored"});
    let (rows, next, end, _) = parse_github(&json!(vec![row; 25]), &r, origin, 1).unwrap();
    assert!(rows.is_empty());
    assert_eq!(next.as_deref(), Some("2"));
    assert!(!end);
}
#[test]
fn discovery_gitlab_rows_require_project_and_state_match() {
    let mut r = request("group/repo");
    r.state = DiscoveryState::Open;
    let (rows, _, _, _) = parse_gitlab(&json!([gitlab_row()]), &r, "7", 1).unwrap();
    assert_eq!(rows[0].number, "42");
    let mut row = gitlab_row();
    row["project_id"] = json!(8);
    assert!(parse_gitlab(&json!([row]), &r, "7", 1).is_err());
    let mut row = gitlab_row();
    row["state"] = json!("closed");
    assert!(parse_gitlab(&json!([row]), &r, "7", 1).is_err());
    let mut row = gitlab_row();
    row["iid"] = json!(0);
    assert!(parse_gitlab(&json!([row]), &r, "7", 1).is_err());
}
#[test]
fn discovery_jira_rejects_moved_keys_foreign_project_and_issuer() {
    let r = request("jira.example/ENG");
    let origin = "https://jira.example";
    let (rows, _, end, _) = parse_jira(&jira_page(), &r, origin, "7", "ENG").unwrap();
    assert_eq!(rows[0].number, "42");
    assert_eq!(
        rows[0].updated_at.as_deref(),
        Some("2026-11-09T10:00:00+00:00")
    );
    assert!(end);
    for (pointer, value) in [
        ("/issues/0/key", "OTHER-42"),
        ("/issues/0/fields/project/key", "OTHER"),
        ("/issues/0/fields/project/id", "8"),
        (
            "/issues/0/self",
            "https://other.example/rest/api/3/issue/1234",
        ),
        ("/issues/0/fields/status/statusCategory/key", "unknown"),
    ] {
        let mut v = jira_page();
        *v.pointer_mut(pointer).unwrap() = json!(value);
        assert!(parse_jira(&v, &r, origin, "7", "ENG").is_err());
    }
}
#[test]
fn discovery_linear_rejects_moved_identifier_and_team() {
    let r = request("ENG");
    let (rows, _, end, _) = parse_linear(&linear_page(), &r, "team-id", "ENG").unwrap();
    assert_eq!(rows[0].number, "42");
    assert!(end);
    for (pointer, value) in [
        ("/team/id", "other-id"),
        ("/team/issues/nodes/0/team/id", "other-id"),
        ("/team/issues/nodes/0/team/key", "OTHER"),
        ("/team/issues/nodes/0/identifier", "OTHER-42"),
        ("/team/issues/nodes/0/state/type", "unknown"),
    ] {
        let mut v = linear_page();
        *v.pointer_mut(pointer).unwrap() = json!(value);
        assert!(parse_linear(&v, &r, "team-id", "ENG").is_err());
    }
}
#[test]
fn discovery_summary_bounds_are_utf8_safe_and_reject_unsafe_urls() {
    let row = summary(
        "18446744073709551615".into(),
        &json!("€".repeat(2000)),
        "open",
        &Value::Null,
        &Value::Null,
    )
    .unwrap();
    assert!(row.title_truncated);
    assert_eq!(row.title.len(), 4095);
    assert_eq!(
        decimal(&json!("18446744073709551615")).unwrap(),
        "18446744073709551615"
    );
    for n in ["0", "01", "18446744073709551616", "-1"] {
        assert!(decimal(&json!(n)).is_err());
    }
    for url in [
        "javascript:alert(1)",
        "https://user:secret@example.com/path",
        "https://example.com/path?token=x",
        "https://example.com/path#fragment",
    ] {
        assert!(summary(
            "1".into(),
            &json!("title"),
            "open",
            &Value::Null,
            &json!(url)
        )
        .is_err());
    }
    assert!(summary(
        "1".into(),
        &json!("title"),
        "open",
        &json!("bad timestamp"),
        &Value::Null
    )
    .is_err());
}
#[test]
fn discovery_continuations_are_bounded_and_do_not_claim_false_exhaustion() {
    assert_eq!(rest_continuation(1, 24), (None, true, false));
    assert_eq!(rest_continuation(1, 25), (Some("2".into()), false, false));
    assert_eq!(rest_continuation(10_000, 25), (None, false, true));
    for token in ["0", "01", "10001", "-1"] {
        assert!(page(Some(token)).is_err());
    }
    assert!(opaque(&json!("a"), true, Some("a")).is_err());
    assert!(opaque(&json!("x".repeat(4097)), true, None).is_err());
    assert!(opaque(&Value::Null, true, None).is_err());
    assert!(rows(&json!(vec![gitlab_row(); 26])).is_err());
    let mut v = jira_page();
    v["isLast"] = json!(false);
    assert!(parse_jira(&v, &request("ENG"), "https://jira.example", "7", "ENG").is_err());
    v["nextPageToken"] = json!("a+b/=");
    assert_eq!(
        parse_jira(&v, &request("ENG"), "https://jira.example", "7", "ENG")
            .unwrap()
            .1
            .as_deref(),
        Some("a+b/=")
    );
}
#[test]
fn discovery_title_query_is_literal_bounded_and_unsupported_where_not_implemented() {
    let mut r = request("group/repo");
    r.title_query = Some("repo:other & state:closed / ? \" literal".into());
    assert!(validate(&r, IssueTracker::Gitlab).is_ok());
    let url = query(
        "/projects/7/issues",
        &[
            ("search", r.title_query.clone().unwrap()),
            ("in", "title".into()),
        ],
    );
    let parsed = url::Url::parse(&format!("https://gitlab.example{url}")).unwrap();
    let values = parsed.query_pairs().collect::<Vec<_>>();
    assert_eq!(values.len(), 2);
    assert_eq!(values[0].1, r.title_query.as_deref().unwrap());
    r.project = "ENG".into();
    assert!(validate(&r, IssueTracker::Linear).is_ok());
    for tracker in [IssueTracker::Github, IssueTracker::Jira] {
        r.project = if tracker == IssueTracker::Github {
            "org/repo"
        } else {
            "ENG"
        }
        .into();
        assert!(validate(&r, tracker)
            .unwrap_err()
            .to_string()
            .contains("title_search_unsupported"));
    }
    r.project = "ENG".into();
    for q in ["x".repeat(129), "\n/merge".into(), "   ".into()] {
        r.title_query = Some(q);
        assert!(validate(&r, IssueTracker::Linear).is_err());
    }
}

#[test]
fn discovery_same_path_replacement_preserves_distinct_provider_object_id() {
    let r = request("org/repo");
    let origin = "https://api.github.example";
    let mut proof =
        json!({"id":7,"full_name":"org/repo","url":"https://api.github.example/repos/org/repo"});
    let first = super::providers::github_project_id(&proof, &r, origin).unwrap();
    proof["id"] = json!(8);
    let replacement = super::providers::github_project_id(&proof, &r, origin).unwrap();
    assert_eq!(first, "7");
    assert_eq!(replacement, "8");
    assert_ne!(first, replacement);
    proof["full_name"] = json!("other/repo");
    assert!(super::providers::github_project_id(&proof, &r, origin).is_err());
}
#[test]
fn discovery_headers_preserve_short_page_continuation_and_scope() {
    use reqwest::header::{HeaderMap, HeaderValue, LINK};
    let origin = "https://api.github.example";
    let path = "/repos/org/repo/issues?per_page=25&page=1&state=all";
    let mut headers = HeaderMap::new();
    headers.insert(LINK, HeaderValue::from_static("<https://api.github.example/repos/org/repo/issues?per_page=25&page=2&state=all>; rel=\"next\""));
    let result = super::providers::header_continuation(
        (Vec::new(), None, true, false),
        &headers,
        origin,
        path,
        1,
        3,
    )
    .unwrap();
    assert_eq!(result.1.as_deref(), Some("2"));
    assert!(!result.2);
    headers.insert(LINK, HeaderValue::from_static("<https://other.example/repos/org/repo/issues?per_page=25&page=2&state=all>; rel=\"next\""));
    assert!(super::providers::header_continuation(
        (Vec::new(), None, true, false),
        &headers,
        origin,
        path,
        1,
        3
    )
    .is_err());
    let result = super::providers::header_continuation(
        (Vec::new(), None, false, false),
        &HeaderMap::new(),
        origin,
        path,
        10_000,
        25,
    )
    .unwrap();
    assert!(result.1.is_none() && !result.2 && result.3);
}
