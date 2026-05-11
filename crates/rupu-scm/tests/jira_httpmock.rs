use base64::engine::general_purpose::STANDARD as Base64;
use base64::Engine;
use httpmock::prelude::*;
use rupu_providers::auth::AuthCredentials;
use rupu_scm::connectors::jira::JiraIssueConnector;
use rupu_scm::connectors::IssueConnector;

fn connector_against(server: &MockServer) -> JiraIssueConnector {
    JiraIssueConnector::new(
        AuthCredentials::ApiKey {
            key: "matt@example.com:api-token".into(),
        },
        Some(server.base_url()),
    )
    .unwrap()
}

fn auth_header() -> String {
    format!(
        "Basic {}",
        Base64.encode("matt@example.com:api-token".as_bytes())
    )
}

fn issue_response(key: &str, status_category_key: &str, labels: &[&str]) -> serde_json::Value {
    serde_json::json!({
        "id": format!("id-{key}"),
        "key": key,
        "self": format!("https://acme.atlassian.net/rest/api/3/issue/{key}"),
        "fields": {
            "summary": format!("Issue {key}"),
            "description": {
                "type": "doc",
                "version": 1,
                "content": [{
                    "type": "paragraph",
                    "content": [{ "type": "text", "text": format!("Description for {key}") }]
                }]
            },
            "labels": labels,
            "status": {
                "id": format!("status-{status_category_key}"),
                "name": if status_category_key == "done" { "Done" } else { "In Progress" },
                "statusCategory": { "key": status_category_key, "name": if status_category_key == "done" { "Done" } else { "In Progress" } }
            },
            "reporter": { "displayName": "matt" },
            "created": "2026-05-10T00:00:00.000+0000",
            "updated": "2026-05-10T01:00:00.000+0000"
        }
    })
}

#[tokio::test]
async fn list_issues_translates_and_filters() {
    let server = MockServer::start_async().await;
    let list = server.mock(|when, then| {
        when.method(POST)
            .path("/rest/api/3/search/jql")
            .header("authorization", auth_header())
            .body_contains("statusCategory != Done")
            .body_contains(
                "\"project = \\\"ENG\\\" AND statusCategory != Done ORDER BY updated DESC\"",
            );
        then.status(200).json_body(serde_json::json!({
            "issues": [
                issue_response("ENG-42", "indeterminate", &["bug"]),
                issue_response("ENG-43", "done", &["chore"])
            ],
            "nextPageToken": null
        }));
    });

    let issues = connector_against(&server)
        .list_issues(
            "ENG",
            rupu_scm::IssueFilter {
                state: Some(rupu_scm::IssueState::Open),
                labels: vec!["bug".into()],
                author: Some("matt".into()),
                limit: Some(10),
            },
        )
        .await
        .unwrap();

    list.assert();
    assert_eq!(issues.len(), 1);
    assert_eq!(issues[0].r.project, "127.0.0.1/ENG");
    assert_eq!(issues[0].r.number, 42);
    assert_eq!(issues[0].title, "Issue ENG-42");
    assert_eq!(issues[0].body, "Description for ENG-42");
    assert_eq!(issues[0].labels, vec!["bug"]);
    assert_eq!(issues[0].author, "matt");
    assert_eq!(issues[0].state, rupu_scm::IssueState::Open);
}

#[tokio::test]
async fn get_issue_translates() {
    let server = MockServer::start_async().await;
    let issue_mock = server.mock(|when, then| {
        when.method(GET)
            .path("/rest/api/3/issue/ENG-42")
            .header("authorization", auth_header());
        then.status(200)
            .json_body(issue_response("ENG-42", "indeterminate", &["bug"]));
    });

    let issue = connector_against(&server)
        .get_issue(&rupu_scm::IssueRef {
            tracker: rupu_scm::IssueTracker::Jira,
            project: "ENG".into(),
            number: 42,
        })
        .await
        .unwrap();

    issue_mock.assert();
    assert_eq!(issue.r.number, 42);
    assert_eq!(issue.title, "Issue ENG-42");
    assert_eq!(issue.body, "Description for ENG-42");
    assert_eq!(issue.state, rupu_scm::IssueState::Open);
}

#[tokio::test]
async fn create_issue_discovers_issue_type_and_fetches_issue() {
    let server = MockServer::start_async().await;
    let types = server.mock(|when, then| {
        when.method(GET)
            .path("/rest/api/3/issue/createmeta/ENG/issuetypes")
            .header("authorization", auth_header());
        then.status(200).json_body(serde_json::json!([
            { "id": "10001", "name": "Task", "subtask": false },
            { "id": "10002", "name": "Sub-task", "subtask": true }
        ]));
    });
    let create = server.mock(|when, then| {
        when.method(POST)
            .path("/rest/api/3/issue")
            .body_contains("\"project\":{\"key\":\"ENG\"}")
            .body_contains("\"issuetype\":{\"id\":\"10001\"}")
            .body_contains("\"labels\":[\"bug\"]")
            .body_contains("\"type\":\"doc\"")
            .header("authorization", auth_header());
        then.status(201).json_body(serde_json::json!({
            "id": "10099",
            "key": "ENG-99"
        }));
    });
    let get_issue = server.mock(|when, then| {
        when.method(GET).path("/rest/api/3/issue/ENG-99");
        then.status(200)
            .json_body(issue_response("ENG-99", "indeterminate", &["bug"]));
    });

    let issue = connector_against(&server)
        .create_issue(
            "ENG",
            rupu_scm::CreateIssue {
                title: "New issue".into(),
                body: "Body".into(),
                labels: vec!["bug".into()],
            },
        )
        .await
        .unwrap();

    types.assert();
    create.assert();
    get_issue.assert();
    assert_eq!(issue.r.number, 99);
    assert_eq!(issue.title, "Issue ENG-99");
}

#[tokio::test]
async fn comment_issue_posts_adf_comment() {
    let server = MockServer::start_async().await;
    let comment_mock = server.mock(|when, then| {
        when.method(POST)
            .path("/rest/api/3/issue/ENG-42/comment")
            .header("authorization", auth_header())
            .body_contains("\"type\":\"doc\"")
            .body_contains("looks good");
        then.status(201).json_body(serde_json::json!({
            "id": "comment-1",
            "body": {
                "type": "doc",
                "version": 1,
                "content": [{
                    "type": "paragraph",
                    "content": [{ "type": "text", "text": "looks good" }]
                }]
            },
            "created": "2026-05-10T02:00:00.000+0000",
            "author": { "displayName": "matt" }
        }));
    });

    let comment = connector_against(&server)
        .comment_issue(
            &rupu_scm::IssueRef {
                tracker: rupu_scm::IssueTracker::Jira,
                project: "ENG".into(),
                number: 42,
            },
            "looks good",
        )
        .await
        .unwrap();

    comment_mock.assert();
    assert_eq!(comment.id, "comment-1");
    assert_eq!(comment.author, "matt");
    assert_eq!(comment.body, "looks good");
}

#[tokio::test]
async fn update_issue_state_uses_done_transition() {
    let server = MockServer::start_async().await;
    let transitions = server.mock(|when, then| {
        when.method(GET)
            .path("/rest/api/3/issue/ENG-42/transitions")
            .header("authorization", auth_header());
        then.status(200).json_body(serde_json::json!({
            "transitions": [
                {
                    "id": "11",
                    "name": "Close Issue",
                    "to": {
                        "id": "10000",
                        "name": "Done",
                        "statusCategory": { "key": "done", "name": "Done" }
                    }
                }
            ]
        }));
    });
    let post_transition = server.mock(|when, then| {
        when.method(POST)
            .path("/rest/api/3/issue/ENG-42/transitions")
            .header("authorization", auth_header())
            .body_contains("\"transition\":{\"id\":\"11\"}");
        then.status(204).json_body(serde_json::json!({}));
    });

    connector_against(&server)
        .update_issue_state(
            &rupu_scm::IssueRef {
                tracker: rupu_scm::IssueTracker::Jira,
                project: "ENG".into(),
                number: 42,
            },
            rupu_scm::IssueState::Closed,
        )
        .await
        .unwrap();

    transitions.assert();
    post_transition.assert();
}
