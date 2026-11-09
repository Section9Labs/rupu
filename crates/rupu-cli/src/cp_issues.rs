//! Operator issue reads over the same instance/account registry as model tools.
//! No caller-supplied API URL reaches a connector; origin is an equality guard.
use crate::cp_repos::CpRepoLister;
use rupu_cp::issue_reader::{
    IssueDiscoveryTarget, IssueReadAccount, IssueReadError as Error, IssueReadTarget, IssueReader,
};
use rupu_scm::{AccountId, IssueConnector, IssueRef, ScmError};
use std::sync::Arc;

fn resolve(
    port: &CpRepoLister,
    target: &IssueReadTarget,
) -> Result<(Arc<dyn IssueConnector>, IssueRef, IssueRef), Error> {
    let original = IssueRef {
        tracker: target.tracker,
        project: target.project.clone(),
        number: target.number,
    };
    rupu_scm::operator_issues::validate_project(&original).map_err(|_| Error::InvalidTarget)?;
    let repo = rupu_scm::tracker_project_repo(target.tracker, &target.project);
    let selected = AccountId::new(&target.account);
    let (account, connector) = port
        .registry
        .issues_for(target.tracker, repo.as_ref(), None, Some(&selected))
        .map_err(|_| Error::AccountUnavailable)?;
    if account != selected || connector.tracker() != target.tracker {
        return Err(Error::SelectionChanged);
    }
    let capabilities = connector.operator_read_capabilities();
    if !capabilities.body_supported {
        return Err(Error::Unsupported);
    }
    if capabilities.origin.as_deref() != Some(target.origin.as_str()) {
        return Err(Error::SelectionChanged);
    }
    // Includes Jira's configured-site equality check before get_issue can attach
    // credentials. Expected response identity is separate from request routing.
    let expected = connector
        .operator_expected_issue(&original)
        .map_err(|_| Error::InvalidTarget)?;
    Ok((connector, original, expected))
}

// Only reuses the existing project/account/origin validator. No issue numbered
// one is fetched and no discovery row derives identity from this sentinel.
fn discovery_validation_target(target: &IssueDiscoveryTarget) -> IssueReadTarget {
    IssueReadTarget {
        tracker: target.tracker,
        project: target.project.clone(),
        account: target.account.clone(),
        origin: target.origin.clone(),
        number: 1,
    }
}

fn discovery_error(error: ScmError) -> Error {
    match &error {
        ScmError::BadRequest { message } if message == "discussion_selection_changed" => {
            Error::SelectionChanged
        }
        ScmError::BadRequest { message }
            if matches!(
                message.as_str(),
                "issue_discovery_title_search_unsupported"
                    | "issue_discovery_unsupported"
                    | "discussion_unsupported"
            ) =>
        {
            Error::Unsupported
        }
        _ => sanitized(error),
    }
}

fn sanitized(error: ScmError) -> Error {
    match error {
        ScmError::NotFound { .. } => Error::NotFound,
        ScmError::Unauthorized { .. }
        | ScmError::MissingScope { .. }
        | ScmError::Forbidden { .. } => Error::Forbidden,
        ScmError::RateLimited { .. } => Error::RateLimited,
        _ => Error::UpstreamUnavailable,
    }
}

#[async_trait::async_trait]
impl IssueReader for CpRepoLister {
    fn supports_discovery(&self) -> bool {
        true
    }
    fn discovery_epoch(&self, target: &IssueDiscoveryTarget) -> Result<String, Error> {
        let (connector, _, _) = resolve(self, &discovery_validation_target(target))?;
        if !connector.discovery_supported() {
            return Err(Error::Unsupported);
        }
        connector.discovery_epoch().ok_or(Error::Unsupported)
    }
    async fn discovery_page(
        &self,
        target: &IssueDiscoveryTarget,
        request: &rupu_scm::issue_discovery::DiscoveryRequest,
    ) -> Result<rupu_scm::issue_discovery::DiscoveryPage, Error> {
        if request.project != target.project {
            return Err(Error::InvalidTarget);
        }
        let (connector, _, expected) = resolve(self, &discovery_validation_target(target))?;
        if !connector.discovery_supported()
            || (request.title_query.is_some() && !connector.discovery_search_supported())
        {
            return Err(Error::Unsupported);
        }
        let before = connector.discovery_epoch().ok_or(Error::Unsupported)?;
        let page = connector
            .discovery_page(request)
            .await
            .map_err(discovery_error)?;
        if connector.discovery_epoch().as_deref() != Some(before.as_str())
            || page.connector_epoch != before
            || page.project != expected.project
        {
            return Err(Error::SelectionChanged);
        }
        Ok(page)
    }

    fn supports_discussion(&self) -> bool {
        true
    }
    fn discussion_epoch(&self, target: &IssueReadTarget) -> Result<String, Error> {
        let (connector, _, _) = resolve(self, target)?;
        if !connector.discussion_supported() {
            return Err(Error::Unsupported);
        }
        connector.discussion_epoch().ok_or(Error::Unsupported)
    }
    async fn discussion_page(
        &self,
        target: &IssueReadTarget,
        after: Option<&str>,
    ) -> Result<rupu_scm::issue_discussion::DiscussionPage, Error> {
        let (connector, original, _) = resolve(self, target)?;
        if !connector.discussion_supported() {
            return Err(Error::Unsupported);
        }
        let before = connector.discussion_epoch().ok_or(Error::Unsupported)?;
        let page = connector
            .discussion_page(&original, after)
            .await
            .map_err(|e| match &e {
                ScmError::BadRequest { message } if message == "discussion_selection_changed" => {
                    Error::SelectionChanged
                }
                ScmError::BadRequest { message } if message == "discussion_unsupported" => {
                    Error::Unsupported
                }
                _ => sanitized(e),
            })?;
        if connector.discussion_epoch().as_deref() != Some(before.as_str())
            || page.identity.connector_epoch != before
        {
            return Err(Error::SelectionChanged);
        }
        Ok(page)
    }

    fn accounts(&self) -> Vec<IssueReadAccount> {
        self.registry
            .all_issue_connectors()
            .into_iter()
            .map(|(account, tracker, connector)| {
                let c = connector.operator_read_capabilities();
                IssueReadAccount {
                    tracker,
                    account: account.to_string(),
                    origin: c.origin,
                    body_supported: c.body_supported,
                    comments_supported: c.comments_supported,
                    discussion_supported: connector.discussion_supported(),
                    discovery_supported: connector.discovery_supported(),
                    discovery_search_supported: connector.discovery_search_supported(),
                    unavailable_reason: c.unavailable_reason,
                }
            })
            .collect()
    }
    fn comments_supported(&self, target: &IssueReadTarget) -> Result<bool, Error> {
        Ok(resolve(self, target)?
            .0
            .operator_read_capabilities()
            .comments_supported)
    }
    async fn issue(&self, target: &IssueReadTarget) -> Result<rupu_scm::Issue, Error> {
        let (connector, original, expected) = resolve(self, target)?;
        let issue = connector
            .operator_get_issue(&original)
            .await
            .map_err(sanitized)?;
        if issue.r != expected {
            return Err(Error::SelectionChanged);
        }
        Ok(issue)
    }
    async fn comments(
        &self,
        target: &IssueReadTarget,
        page: u32,
    ) -> Result<rupu_scm::operator_issues::IssueCommentPage, Error> {
        let (connector, original, _) = resolve(self, target)?;
        if !connector.operator_read_capabilities().comments_supported {
            return Err(Error::Unsupported);
        }
        connector
            .operator_comments_page(&original, page)
            .await
            .map_err(sanitized)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rupu_scm::{
        Comment, CreateIssue, Issue, IssueFilter, IssueState, IssueTracker, Platform, Registry,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Fake {
        calls: Arc<AtomicUsize>,
        mismatch: bool,
    }
    #[async_trait::async_trait]
    impl IssueConnector for Fake {
        fn tracker(&self) -> IssueTracker {
            IssueTracker::Github
        }
        fn operator_read_capabilities(&self) -> rupu_scm::operator_issues::IssueReadCapabilities {
            rupu_scm::operator_issues::IssueReadCapabilities::configured(
                Some("https://api.github.com".into()),
                false,
            )
        }
        fn discovery_supported(&self) -> bool {
            true
        }
        fn discovery_search_supported(&self) -> bool {
            false
        }
        fn discovery_epoch(&self) -> Option<String> {
            Some("fixture-epoch".into())
        }
        async fn discovery_page(
            &self,
            request: &rupu_scm::issue_discovery::DiscoveryRequest,
        ) -> Result<rupu_scm::issue_discovery::DiscoveryPage, ScmError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(rupu_scm::issue_discovery::DiscoveryPage {
                project: if self.mismatch {
                    "other/repo".into()
                } else {
                    request.project.clone()
                },
                provider_project_id: "repo-1".into(),
                connector_epoch: "fixture-epoch".into(),
                rows: vec![],
                next: None,
                end_observed: true,
                truncated: false,
                search_mode: "none".into(),
            })
        }
        async fn operator_get_issue(&self, i: &IssueRef) -> Result<Issue, ScmError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let mut r = i.clone();
            if self.mismatch {
                r.number += 1;
            }
            Ok(Issue {
                r,
                title: "fixture".into(),
                body: "body".into(),
                state: IssueState::Open,
                labels: vec![],
                label_colors: Default::default(),
                author: "operator".into(),
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
            })
        }
        async fn get_issue(&self, _: &IssueRef) -> Result<Issue, ScmError> {
            unreachable!()
        }
        async fn list_issues(&self, _: &str, _: IssueFilter) -> Result<Vec<Issue>, ScmError> {
            unreachable!()
        }
        async fn comment_issue(&self, _: &IssueRef, _: &str) -> Result<Comment, ScmError> {
            unreachable!()
        }
        async fn create_issue(&self, _: &str, _: CreateIssue) -> Result<Issue, ScmError> {
            unreachable!()
        }
        async fn update_issue_state(&self, _: &IssueRef, _: IssueState) -> Result<(), ScmError> {
            unreachable!()
        }
    }
    fn target() -> IssueReadTarget {
        IssueReadTarget {
            tracker: IssueTracker::Github,
            project: "org/repo".into(),
            account: "work".into(),
            number: 1,
            origin: "https://api.github.com".into(),
        }
    }
    #[tokio::test]
    async fn explicit_account_origin_and_identity_are_enforced_before_read() {
        let work = Arc::new(AtomicUsize::new(0));
        let personal = Arc::new(AtomicUsize::new(0));
        let mut registry = Registry::default();
        registry.insert_issue_account(
            AccountId::new("work"),
            Platform::Github,
            Arc::new(Fake {
                calls: work.clone(),
                mismatch: false,
            }),
        );
        registry.insert_issue_account(
            AccountId::new("personal"),
            Platform::Github,
            Arc::new(Fake {
                calls: personal.clone(),
                mismatch: true,
            }),
        );
        let port = CpRepoLister {
            registry: Arc::new(registry),
        };
        let mut selected = target();
        selected.origin = "https://wrong.example".into();
        assert!(matches!(
            port.issue(&selected).await,
            Err(Error::SelectionChanged)
        ));
        selected = target();
        selected.account = "absent".into();
        assert!(matches!(
            port.issue(&selected).await,
            Err(Error::AccountUnavailable)
        ));
        assert_eq!(work.load(Ordering::SeqCst), 0);
        assert_eq!(personal.load(Ordering::SeqCst), 0);
        port.issue(&target()).await.unwrap();
        assert_eq!(work.load(Ordering::SeqCst), 1);
        assert_eq!(personal.load(Ordering::SeqCst), 0);
        selected = target();
        selected.account = "personal".into();
        assert!(matches!(
            port.issue(&selected).await,
            Err(Error::SelectionChanged)
        ));
        assert_eq!(personal.load(Ordering::SeqCst), 1);
        assert!(!port.comments_supported(&target()).unwrap());
    }
    #[tokio::test]
    async fn discovery_keeps_exact_account_origin_project_and_search_capability_before_fetch() {
        let calls = Arc::new(AtomicUsize::new(0));
        let mut registry = Registry::default();
        registry.insert_issue_account(
            AccountId::new("work"),
            Platform::Github,
            Arc::new(Fake {
                calls: calls.clone(),
                mismatch: false,
            }),
        );
        registry.insert_issue_account(
            AccountId::new("wrong"),
            Platform::Github,
            Arc::new(Fake {
                calls: calls.clone(),
                mismatch: true,
            }),
        );
        let port = CpRepoLister {
            registry: Arc::new(registry),
        };
        let mut target = IssueDiscoveryTarget {
            tracker: IssueTracker::Github,
            project: "org/repo".into(),
            account: "work".into(),
            origin: "https://api.github.com".into(),
        };
        let mut request = rupu_scm::issue_discovery::DiscoveryRequest {
            project: target.project.clone(),
            state: rupu_scm::issue_discovery::DiscoveryState::All,
            title_query: None,
            after: None,
        };
        target.origin = "https://other.example".into();
        assert!(matches!(
            port.discovery_page(&target, &request).await,
            Err(Error::SelectionChanged)
        ));
        target.origin = "https://api.github.com".into();
        request.title_query = Some("search".into());
        assert!(matches!(
            port.discovery_page(&target, &request).await,
            Err(Error::Unsupported)
        ));
        request.title_query = None;
        request.project = "other/repo".into();
        assert!(matches!(
            port.discovery_page(&target, &request).await,
            Err(Error::InvalidTarget)
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        request.project = target.project.clone();
        port.discovery_page(&target, &request).await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        target.account = "wrong".into();
        assert!(matches!(
            port.discovery_page(&target, &request).await,
            Err(Error::SelectionChanged)
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        let account = port
            .accounts()
            .into_iter()
            .find(|a| a.account == "work")
            .unwrap();
        assert!(account.discovery_supported);
        assert!(!account.discovery_search_supported);
    }
    #[test]
    fn discovery_preserves_typed_unsupported_and_changed_without_provider_text() {
        for message in [
            "issue_discovery_title_search_unsupported",
            "issue_discovery_unsupported",
            "discussion_unsupported",
        ] {
            assert!(matches!(
                discovery_error(ScmError::BadRequest {
                    message: message.into()
                }),
                Error::Unsupported
            ));
        }
        assert!(matches!(
            discovery_error(ScmError::BadRequest {
                message: "discussion_selection_changed".into()
            }),
            Error::SelectionChanged
        ));
        assert!(matches!(
            discovery_error(ScmError::BadRequest {
                message: "secret upstream payload".into()
            }),
            Error::UpstreamUnavailable
        ));
    }

    #[test]
    fn upstream_error_payloads_never_enter_public_error() {
        let error = sanitized(ScmError::BadRequest {
            message: "https://user:secret@private.example?token=secret".into(),
        });
        assert!(matches!(error, Error::UpstreamUnavailable));
    }
}
