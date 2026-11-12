//! Narrow operator read surface. Existing runtime issue tools keep their own
//! behavior; operator reads require a configured, non-secret API authority.
use crate::{Comment, IssueRef, IssueTracker, ScmError};
use serde::Serialize;

#[derive(Clone, Debug, Serialize)]
pub struct IssueReadCapabilities {
    pub origin: Option<String>,
    pub body_supported: bool,
    pub comments_supported: bool,
    pub unavailable_reason: Option<String>,
}

impl IssueReadCapabilities {
    pub fn configured(origin: Option<String>, comments: bool) -> Self {
        let available = origin.is_some();
        Self {
            origin,
            body_supported: available,
            comments_supported: available && comments,
            unavailable_reason: (!available)
                .then(|| "A safe configured API origin is required".into()),
        }
    }
}

pub struct IssueCommentPage {
    pub items: Vec<Comment>,
    /// A full page is only a possible continuation, not a complete-count claim.
    pub next_page: Option<u32>,
    pub end_observed: bool,
}

/// Preserve configured scheme, host, port and path. Never repair or strip a
/// credential-bearing URL into an apparently safe selection.
pub fn safe_origin(value: &str) -> Option<String> {
    let mut url = url::Url::parse(value).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let path = url.path().trim_end_matches('/').to_owned();
    url.set_path(&path);
    Some(url.as_str().trim_end_matches('/').to_owned())
}

pub fn validate_project(issue: &IssueRef) -> Result<(), ScmError> {
    let valid_component = |s: &str| {
        !s.is_empty()
            && s != "."
            && s != ".."
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
    };
    let parts: Vec<_> = issue.project.split('/').collect();
    let valid = issue.number > 0
        && issue.project.len() <= 512
        && match issue.tracker {
            IssueTracker::Github => parts.len() == 2 && parts.iter().all(|s| valid_component(s)),
            IssueTracker::Gitlab => parts.len() >= 2 && parts.iter().all(|s| valid_component(s)),
            IssueTracker::Linear => parts.len() == 1 && valid_component(&issue.project),
            // Jira's configured-origin check must run in its connector before auth.
            IssueTracker::Jira => {
                !issue.project.is_empty() && !issue.project.chars().any(char::is_control)
            }
        };
    if valid {
        Ok(())
    } else {
        Err(ScmError::BadRequest {
            message: "Invalid exact issue project".into(),
        })
    }
}

/// Dedicated operator transport: no redirect can change authority after auth.
pub(crate) fn http(
    tracker: &str,
    sink: std::sync::Arc<dyn rupu_netflow::FlowSink>,
) -> anyhow::Result<reqwest_middleware::ClientWithMiddleware> {
    Ok(rupu_netflow::http::client_with(
        rupu_netflow::FlowCtx::system(rupu_netflow::Origin::Scm(tracker.into())),
        reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(std::time::Duration::from_secs(10))
            .user_agent("rupu-operator-issues"),
        sink,
    )?)
}

pub(crate) fn rejected() -> ScmError {
    ScmError::BadRequest {
        message: "Operator issue response failed authority or size validation".into(),
    }
}

/// Bound successful and error bodies before JSON decoding. Operator adapters do
/// at most two issue requests and one comment request, each at most 2 MiB.
pub(crate) async fn json(mut response: reqwest::Response) -> Result<serde_json::Value, ScmError> {
    const MAX_BYTES: usize = 2 * 1024 * 1024;
    let status = response.status().as_u16();
    if response
        .content_length()
        .is_some_and(|n| n > MAX_BYTES as u64)
    {
        return Err(rejected());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| rejected())? {
        if chunk.len() > MAX_BYTES.saturating_sub(bytes.len()) {
            return Err(rejected());
        }
        bytes.extend_from_slice(&chunk);
    }
    match status {
        200..=299 => serde_json::from_slice(&bytes).map_err(|_| rejected()),
        401 | 403 => Err(ScmError::Forbidden {
            platform: "issue".into(),
            message: "Issue account refused".into(),
        }),
        404 => Err(ScmError::NotFound {
            what: "issue".into(),
        }),
        429 => Err(ScmError::RateLimited { retry_after: None }),
        _ => Err(rejected()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn bounded_transport_rejects_oversized_bodies_and_sanitizes_errors() {
        let response = |status: u16, body: String| {
            reqwest::Response::from(
                ::http::Response::builder()
                    .status(status)
                    .body(body)
                    .unwrap(),
            )
        };
        assert!(json(response(200, "x".repeat(2 * 1024 * 1024 + 1)))
            .await
            .is_err());
        assert_eq!(
            json(response(200, "{\"ok\":true}".into())).await.unwrap()["ok"],
            true
        );
        let error = json(response(403, "secret credential payload".into()))
            .await
            .err()
            .unwrap();
        assert!(matches!(&error, ScmError::Forbidden { .. }));
        assert!(!error.to_string().contains("secret"));
        assert!(json(response(302, "{}".into())).await.is_err());
    }

    #[test]
    fn origin_preserves_configured_authority_and_rejects_secret_components() {
        assert_eq!(
            safe_origin("https://forge.example:8443/api/v4/"),
            Some("https://forge.example:8443/api/v4".into())
        );
        for value in [
            "https://user:secret@forge.example",
            "https://forge.example?token=secret",
            "https://forge.example/#secret",
            "file:///tmp/forge",
            "https://user@forge.example",
        ] {
            assert_eq!(safe_origin(value), None, "{value}");
        }
    }

    #[test]
    fn exact_project_validation_does_not_resolve_names_or_paths() {
        for (tracker, project, valid) in [
            (IssueTracker::Github, "org/repo", true),
            (IssueTracker::Github, "repo", false),
            (IssueTracker::Github, "org/../repo", false),
            (IssueTracker::Gitlab, "group/subgroup/repo", true),
            (IssueTracker::Gitlab, "group/repo?token=x", false),
            (IssueTracker::Linear, "ENG", true),
            (IssueTracker::Linear, "workspace/team", false),
        ] {
            let issue = IssueRef {
                tracker,
                project: project.into(),
                number: 1,
            };
            assert_eq!(validate_project(&issue).is_ok(), valid);
        }
    }
}
