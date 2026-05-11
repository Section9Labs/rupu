use httpmock::prelude::*;
use rupu_scm::connectors::linear::LinearIssueConnector;
use rupu_scm::connectors::IssueConnector;

fn connector_against(server: &MockServer) -> LinearIssueConnector {
    LinearIssueConnector::new("lin_api_test".into(), Some(server.base_url()))
}

fn issue_node(identifier: &str, state_type: &str, label_name: &str) -> serde_json::Value {
    serde_json::json!({
        "id": format!("uuid-{identifier}"),
        "identifier": identifier,
        "url": format!("https://linear.app/acme/issue/{identifier}"),
        "title": format!("Issue {identifier}"),
        "description": format!("Description for {identifier}"),
        "createdAt": "2026-05-10T00:00:00Z",
        "updatedAt": "2026-05-10T01:00:00Z",
        "creator": { "name": "matt" },
        "team": { "id": "team-123", "key": "ENG", "name": "Engineering" },
        "state": { "id": format!("state-{state_type}"), "name": state_type, "type": state_type },
        "labels": {
            "nodes": [
                { "id": format!("label-{label_name}"), "name": label_name, "color": "ff0000" }
            ]
        }
    })
}

fn team_metadata_response() -> serde_json::Value {
    serde_json::json!({
        "data": {
            "team": {
                "id": "team-123",
                "key": "ENG",
                "states": {
                    "nodes": [
                        { "id": "state-started", "name": "In Progress", "type": "started" },
                        { "id": "state-completed", "name": "Done", "type": "completed" }
                    ]
                },
                "labels": {
                    "nodes": [
                        { "id": "label-bug", "name": "bug", "color": "ff0000" },
                        { "id": "label-chore", "name": "chore", "color": "00ff00" }
                    ]
                }
            }
        }
    })
}

#[tokio::test]
async fn list_issues_translates_and_filters() {
    let server = MockServer::start_async().await;
    let mock = server.mock(|when, then| {
        when.method(POST)
            .path("/")
            .header("authorization", "lin_api_test")
            .body_contains("query TeamIssues");
        then.status(200).json_body(serde_json::json!({
            "data": {
                "team": {
                    "id": "team-123",
                    "key": "ENG",
                    "issues": {
                        "nodes": [
                            issue_node("ENG-42", "started", "bug"),
                            issue_node("ENG-43", "completed", "chore")
                        ],
                        "pageInfo": { "hasNextPage": false, "endCursor": null }
                    }
                }
            }
        }));
    });

    let issues = connector_against(&server)
        .list_issues(
            "team-123",
            rupu_scm::IssueFilter {
                state: Some(rupu_scm::IssueState::Open),
                labels: vec!["bug".into()],
                author: Some("matt".into()),
                limit: Some(10),
            },
        )
        .await
        .unwrap();

    mock.assert();
    assert_eq!(issues.len(), 1);
    assert_eq!(issues[0].r.project, "team-123");
    assert_eq!(issues[0].r.number, 42);
    assert_eq!(issues[0].title, "Issue ENG-42");
    assert_eq!(issues[0].labels, vec!["bug"]);
    assert_eq!(issues[0].label_colors.get("bug").unwrap(), "ff0000");
    assert_eq!(issues[0].author, "matt");
}

#[tokio::test]
async fn get_issue_translates() {
    let server = MockServer::start_async().await;
    let meta = server.mock(|when, then| {
        when.method(POST)
            .path("/")
            .body_contains("query TeamMetadata");
        then.status(200).json_body(team_metadata_response());
    });
    let issue_mock = server.mock(|when, then| {
        when.method(POST).path("/").body_contains("query Issue");
        then.status(200).json_body(serde_json::json!({
            "data": { "issue": issue_node("ENG-42", "started", "bug") }
        }));
    });

    let issue = connector_against(&server)
        .get_issue(&rupu_scm::IssueRef {
            tracker: rupu_scm::IssueTracker::Linear,
            project: "team-123".into(),
            number: 42,
        })
        .await
        .unwrap();

    meta.assert();
    issue_mock.assert();
    assert_eq!(issue.r.number, 42);
    assert_eq!(issue.title, "Issue ENG-42");
    assert_eq!(issue.body, "Description for ENG-42");
    assert_eq!(issue.state, rupu_scm::IssueState::Open);
}

#[tokio::test]
async fn create_issue_posts_team_and_labels() {
    let server = MockServer::start_async().await;
    let meta = server.mock(|when, then| {
        when.method(POST)
            .path("/")
            .body_contains("query TeamMetadata");
        then.status(200).json_body(team_metadata_response());
    });
    let create = server.mock(|when, then| {
        when.method(POST)
            .path("/")
            .body_contains("mutation IssueCreate")
            .body_contains("\"teamId\":\"team-123\"")
            .body_contains("label-bug");
        then.status(200).json_body(serde_json::json!({
            "data": {
                "issueCreate": {
                    "success": true,
                    "issue": issue_node("ENG-99", "started", "bug")
                }
            }
        }));
    });

    let issue = connector_against(&server)
        .create_issue(
            "team-123",
            rupu_scm::CreateIssue {
                title: "New issue".into(),
                body: "Body".into(),
                labels: vec!["bug".into()],
            },
        )
        .await
        .unwrap();

    meta.assert();
    create.assert();
    assert_eq!(issue.r.number, 99);
    assert_eq!(issue.title, "Issue ENG-99");
}

#[tokio::test]
async fn comment_issue_creates_comment() {
    let server = MockServer::start_async().await;
    let meta = server.mock(|when, then| {
        when.method(POST)
            .path("/")
            .body_contains("query TeamMetadata");
        then.status(200).json_body(team_metadata_response());
    });
    let get_issue = server.mock(|when, then| {
        when.method(POST).path("/").body_contains("query Issue");
        then.status(200).json_body(serde_json::json!({
            "data": { "issue": issue_node("ENG-42", "started", "bug") }
        }));
    });
    let comment_mock = server.mock(|when, then| {
        when.method(POST)
            .path("/")
            .body_contains("mutation CommentCreate")
            .body_contains("uuid-ENG-42");
        then.status(200).json_body(serde_json::json!({
            "data": {
                "commentCreate": {
                    "success": true,
                    "comment": {
                        "id": "comment-1",
                        "body": "looks good",
                        "createdAt": "2026-05-10T02:00:00Z",
                        "user": { "name": "matt" }
                    }
                }
            }
        }));
    });

    let comment = connector_against(&server)
        .comment_issue(
            &rupu_scm::IssueRef {
                tracker: rupu_scm::IssueTracker::Linear,
                project: "team-123".into(),
                number: 42,
            },
            "looks good",
        )
        .await
        .unwrap();

    meta.assert();
    get_issue.assert();
    comment_mock.assert();
    assert_eq!(comment.id, "comment-1");
    assert_eq!(comment.author, "matt");
    assert_eq!(comment.body, "looks good");
}

#[tokio::test]
async fn update_issue_state_uses_team_state_mapping() {
    let server = MockServer::start_async().await;
    let meta = server.mock(|when, then| {
        when.method(POST)
            .path("/")
            .body_contains("query TeamMetadata");
        then.status(200).json_body(team_metadata_response());
    });
    let update = server.mock(|when, then| {
        when.method(POST)
            .path("/")
            .body_contains("mutation IssueUpdate")
            .body_contains("ENG-42")
            .body_contains("state-completed");
        then.status(200).json_body(serde_json::json!({
            "data": {
                "issueUpdate": {
                    "success": true,
                    "issue": { "id": "uuid-ENG-42" }
                }
            }
        }));
    });

    connector_against(&server)
        .update_issue_state(
            &rupu_scm::IssueRef {
                tracker: rupu_scm::IssueTracker::Linear,
                project: "team-123".into(),
                number: 42,
            },
            rupu_scm::IssueState::Closed,
        )
        .await
        .unwrap();

    meta.assert();
    update.assert();
}
