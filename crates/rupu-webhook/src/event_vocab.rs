//! Mapping from raw vendor webhook (event name + payload action)
//! to a stable rupu event identifier matched against the workflow's
//! `trigger.event:` field.
//!
//! The vocabulary is dotted: `<vendor>.<noun>.<verb>`. Examples:
//!
//!   github.pr.opened
//!   github.pr.merged
//!   github.pr.closed
//!   github.pr.review_requested
//!   github.issue.opened
//!   github.issue.closed
//!   github.issue.commented
//!   github.push
//!   gitlab.mr.opened
//!   gitlab.mr.merged
//!   gitlab.issue.opened
//!   gitlab.push
//!
//! `*` glob-matching on the workflow side (e.g.
//! `trigger.event: github.issue.*`) is **not yet supported** —
//! workflows must declare the exact vendor event name. Glob support
//! is a 5-line follow-up if needed.

use serde_json::Value;

/// Map a GitHub webhook delivery to a rupu event id. `event_header`
/// is `X-GitHub-Event` (e.g. `pull_request`, `issues`, `push`); the
/// payload's `action` field disambiguates within the noun.
///
/// Returns `None` for events we don't yet recognize — the receiver
/// answers 200 OK but doesn't fire any workflow.
pub fn map_github_event(event_header: &str, payload: &Value) -> Option<String> {
    let action = payload.get("action").and_then(|v| v.as_str());
    match (event_header, action) {
        ("pull_request", Some("opened")) => Some("github.pr.opened".into()),
        ("pull_request", Some("reopened")) => Some("github.pr.reopened".into()),
        ("pull_request", Some("closed")) => {
            // GitHub fires `closed` for both merged and merely-closed
            // PRs; differentiate by checking `pull_request.merged`.
            let merged = payload
                .get("pull_request")
                .and_then(|p| p.get("merged"))
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            Some(if merged {
                "github.pr.merged".into()
            } else {
                "github.pr.closed".into()
            })
        }
        ("pull_request", Some("review_requested")) => Some("github.pr.review_requested".into()),
        ("pull_request", Some("synchronize")) => Some("github.pr.updated".into()),
        ("pull_request_review", Some("submitted")) => Some("github.pr.review_submitted".into()),
        ("issues", Some("opened")) => Some("github.issue.opened".into()),
        ("issues", Some("closed")) => Some("github.issue.closed".into()),
        ("issues", Some("reopened")) => Some("github.issue.reopened".into()),
        ("issues", Some("edited")) => Some("github.issue.edited".into()),
        ("issues", Some("labeled")) => Some("github.issue.labeled".into()),
        ("issues", Some("assigned")) => Some("github.issue.assigned".into()),
        ("issue_comment", Some("created")) => Some("github.issue.commented".into()),
        ("push", _) => Some("github.push".into()),
        ("ping", _) => Some("github.ping".into()),
        _ => None,
    }
}

/// Map a GitLab webhook delivery to a rupu event id. GitLab uses
/// `X-Gitlab-Event` (e.g. `Merge Request Hook`) AND the payload's
/// `object_attributes.action` to disambiguate.
pub fn map_gitlab_event(event_header: &str, payload: &Value) -> Option<String> {
    let action = payload
        .get("object_attributes")
        .and_then(|o| o.get("action"))
        .and_then(|v| v.as_str());
    match (event_header, action) {
        ("Merge Request Hook", Some("open")) => Some("gitlab.mr.opened".into()),
        ("Merge Request Hook", Some("reopen")) => Some("gitlab.mr.reopened".into()),
        ("Merge Request Hook", Some("close")) => Some("gitlab.mr.closed".into()),
        ("Merge Request Hook", Some("merge")) => Some("gitlab.mr.merged".into()),
        ("Merge Request Hook", Some("update")) => Some("gitlab.mr.updated".into()),
        ("Issue Hook", Some("open")) => Some("gitlab.issue.opened".into()),
        ("Issue Hook", Some("close")) => Some("gitlab.issue.closed".into()),
        ("Issue Hook", Some("reopen")) => Some("gitlab.issue.reopened".into()),
        ("Issue Hook", Some("update")) => Some("gitlab.issue.updated".into()),
        ("Note Hook", _) => Some("gitlab.comment".into()),
        ("Push Hook", _) => Some("gitlab.push".into()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn github_pr_opened() {
        let payload = json!({ "action": "opened", "pull_request": { "number": 1 } });
        assert_eq!(
            map_github_event("pull_request", &payload),
            Some("github.pr.opened".into())
        );
    }

    #[test]
    fn github_pr_closed_distinguishes_merged_vs_closed() {
        let merged = json!({ "action": "closed", "pull_request": { "merged": true } });
        let closed = json!({ "action": "closed", "pull_request": { "merged": false } });
        assert_eq!(
            map_github_event("pull_request", &merged),
            Some("github.pr.merged".into())
        );
        assert_eq!(
            map_github_event("pull_request", &closed),
            Some("github.pr.closed".into())
        );
    }

    #[test]
    fn github_issue_events() {
        for (action, expected) in [
            ("opened", "github.issue.opened"),
            ("closed", "github.issue.closed"),
            ("labeled", "github.issue.labeled"),
        ] {
            let payload = json!({ "action": action });
            assert_eq!(
                map_github_event("issues", &payload),
                Some(expected.into()),
                "for action={action}"
            );
        }
    }

    #[test]
    fn github_unknown_event_is_none() {
        let payload = json!({ "action": "speculated" });
        assert!(map_github_event("never_heard_of_it", &payload).is_none());
    }

    #[test]
    fn github_push_does_not_require_action() {
        let payload = json!({ "ref": "refs/heads/main" });
        assert_eq!(
            map_github_event("push", &payload),
            Some("github.push".into())
        );
    }

    #[test]
    fn gitlab_mr_opened() {
        let payload = json!({ "object_attributes": { "action": "open" } });
        assert_eq!(
            map_gitlab_event("Merge Request Hook", &payload),
            Some("gitlab.mr.opened".into())
        );
    }

    #[test]
    fn gitlab_unknown_event() {
        let payload = json!({});
        assert!(map_gitlab_event("Magic Hook", &payload).is_none());
    }
}
