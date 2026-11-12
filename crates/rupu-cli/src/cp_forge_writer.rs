//! Operator-only preparation and single-dispatch adapters over the owning registry.
use crate::cp_repos::CpRepoLister;
use rupu_cp::forge_writer::ForgeWriter;
use rupu_scm::{forge_write::*, AccountId, IssueConnector, IssueRef, IssueTracker, Platform};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::sync::Arc;

fn resolve(
    port: &CpRepoLister,
    request: &ForgeRequest,
    owner: Option<&ForgeOwner>,
) -> Result<Arc<dyn IssueConnector>, ForgeError> {
    validate_request(request)?;
    let account = AccountId::new(request.target.account.clone());
    let repo = rupu_scm::tracker_project_repo(request.target.provider, &request.target.project_ref);
    if request.action.is_repository() {
        let owner = owner.ok_or_else(ForgeError::invalid)?;
        if request.target.ws_id.as_deref() != Some(owner.ws_id.as_str())
            || repo.as_ref() != Some(&owner.repo)
        {
            return Err(ForgeError::changed());
        }
        let (resolved, _) = port
            .registry
            .repo_for(&owner.repo, Some(&owner.canonical_root), None)
            .map_err(|_| {
                ForgeError::fixed(
                    "account_unavailable",
                    "The owning repository account is unavailable",
                )
            })?;
        if resolved != account {
            return Err(ForgeError::changed());
        }
        let origin_matches = match owner.repo.platform {
            Platform::Github => port
                .registry
                .github_extras_for(&owner.repo, Some(&owner.canonical_root), Some(&account))
                .map(|(id, e)| {
                    id == account && e.review_matches_origin(&owner.origin_url_identity)
                }),
            Platform::Gitlab => port
                .registry
                .gitlab_extras_for(&owner.repo, Some(&owner.canonical_root), Some(&account))
                .map(|(id, e)| {
                    id == account && e.review_matches_origin(&owner.origin_url_identity)
                }),
        }
        .map_err(|_| ForgeError::unsupported())?;
        if !origin_matches {
            return Err(ForgeError::changed());
        }
    } else if owner.is_some() {
        return Err(ForgeError::invalid());
    }
    let (resolved, connector) = port
        .registry
        .issues_for(request.target.provider, repo.as_ref(), None, Some(&account))
        .map_err(|_| {
            ForgeError::fixed(
                "account_unavailable",
                "The selected issue adapter is unavailable",
            )
        })?;
    if resolved != account || connector.tracker() != request.target.provider {
        return Err(ForgeError::changed());
    }
    if !connector.forge_actions().contains(&request.action) {
        return Err(ForgeError::unsupported());
    }
    if connector.operator_read_capabilities().origin.as_deref()
        != Some(request.target.api_origin.as_str())
    {
        return Err(ForgeError::changed());
    }
    // Jira's site-qualified input cannot retarget credentials. Run this guard
    // even for creation (number one is validation only, never fetched).
    connector
        .operator_expected_issue(&IssueRef {
            tracker: request.target.provider,
            project: request.target.project_ref.clone(),
            number: request
                .target
                .number
                .as_deref()
                .unwrap_or("1")
                .parse()
                .map_err(|_| ForgeError::invalid())?,
        })
        .map_err(|_| ForgeError::invalid())?;
    Ok(connector)
}
async fn check_issue_version(
    connector: &dyn IssueConnector,
    request: &ForgeRequest,
) -> Result<(), ForgeError> {
    if !matches!(
        request.action,
        ForgeAction::IssueComment | ForgeAction::IssueSetState
    ) {
        return Ok(());
    }
    let expected = request
        .expected
        .get("version")
        .ok_or_else(ForgeError::invalid)?;
    let issue = connector
        .operator_get_issue(&IssueRef {
            tracker: request.target.provider,
            project: request.target.project_ref.clone(),
            number: request
                .target
                .number
                .as_deref()
                .ok_or_else(ForgeError::invalid)?
                .parse()
                .map_err(|_| ForgeError::invalid())?,
        })
        .await
        .map_err(|_| ForgeError::upstream())?;
    let version = format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&issue).map_err(|_| ForgeError::upstream())?)
    );
    if &version != expected {
        return Err(ForgeError::changed());
    }
    Ok(())
}
fn owner_authority(
    prepared: &mut PreparedWrite,
    owner: Option<&ForgeOwner>,
) -> Result<(), ForgeError> {
    if prepared.request.action.is_repository() {
        let owner = owner.ok_or_else(ForgeError::invalid)?;
        if prepared.request.action.is_review()
            && (prepared.observed["head_sha"].as_str() != Some(owner.head.as_str())
                || prepared.observed["head_branch"].as_str() != Some(owner.branch.as_str()))
        {
            return Err(ForgeError::changed());
        }
        prepared.authority["ws_id"] = json!(owner.ws_id);
        prepared.authority["checkout_root_identity"] = json!(owner.checkout_identity);
        prepared.authority["repo_ref"] = json!(format!("{}/{}", owner.repo.owner, owner.repo.repo));
        prepared.authority["origin_url_identity"] = json!(owner.origin_url_identity);
        prepared.authority["checkout_branch"] = json!(owner.branch);
        prepared.authority["checkout_head"] = json!(owner.head);
    }
    Ok(())
}
#[async_trait::async_trait]
impl ForgeWriter for CpRepoLister {
    fn capabilities(&self) -> Vec<ForgeCapability> {
        self.registry
            .all_issue_connectors()
            .into_iter()
            .map(|(account, provider, connector)| {
                let origin = connector.operator_read_capabilities().origin;
                let mut actions = if origin.is_some() {
                    connector.forge_actions()
                } else {
                    vec![]
                };
                let platform = match provider {
                    IssueTracker::Github => Some(Platform::Github),
                    IssueTracker::Gitlab => Some(Platform::Gitlab),
                    _ => None,
                };
                let repo_available = platform.is_some_and(|platform| {
                    self.registry
                        .all_repo_connectors(platform)
                        .iter()
                        .any(|(id, _)| id == &account)
                });
                if !repo_available {
                    actions.retain(|a| !a.is_repository());
                }
                ForgeCapability {
                    provider,
                    account: account.to_string(),
                    api_origin: origin,
                    unavailable_reason: actions
                        .is_empty()
                        .then(|| "No safe owning operator write adapter is configured".into()),
                    actions,
                }
            })
            .collect()
    }
    async fn prepare(
        &self,
        request: &ForgeRequest,
        owner: Option<&ForgeOwner>,
    ) -> Result<PreparedWrite, ForgeError> {
        let connector = resolve(self, request, owner)?;
        check_issue_version(connector.as_ref(), request).await?;
        let session = connector.forge_session().await?;
        let mut prepared = session.prepare(request).await?;
        check_issue_version(connector.as_ref(), request).await?;
        if !session.current() {
            return Err(ForgeError::changed());
        }
        owner_authority(&mut prepared, owner)?;
        Ok(prepared)
    }
    async fn preflight(
        &self,
        intent: &PreparedWrite,
        owner: Option<&ForgeOwner>,
    ) -> Result<Box<dyn PreparedMutation>, ForgeError> {
        let connector = resolve(self, &intent.request, owner)?;
        let mut owner_check = intent.clone();
        owner_authority(&mut owner_check, owner)?;
        if owner_check.authority != intent.authority {
            return Err(ForgeError::changed());
        }
        check_issue_version(connector.as_ref(), &intent.request).await?;
        let session = connector.forge_session().await?;
        let mutation = session.preflight(intent).await?;
        check_issue_version(connector.as_ref(), &intent.request).await?;
        if !mutation.is_current() {
            return Err(ForgeError::changed());
        }
        Ok(mutation)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rupu_scm::{Comment, CreateIssue, Issue, IssueFilter, IssueState, Registry};
    struct Adapter;
    #[async_trait::async_trait]
    impl IssueConnector for Adapter {
        fn tracker(&self) -> IssueTracker {
            IssueTracker::Github
        }
        fn operator_read_capabilities(&self) -> rupu_scm::operator_issues::IssueReadCapabilities {
            rupu_scm::operator_issues::IssueReadCapabilities::configured(
                Some("https://api.github.com".into()),
                false,
            )
        }
        fn forge_actions(&self) -> Vec<ForgeAction> {
            vec![ForgeAction::IssueCreate]
        }
        async fn list_issues(
            &self,
            _: &str,
            _: IssueFilter,
        ) -> Result<Vec<Issue>, rupu_scm::ScmError> {
            unreachable!()
        }
        async fn get_issue(&self, _: &IssueRef) -> Result<Issue, rupu_scm::ScmError> {
            unreachable!()
        }
        async fn comment_issue(
            &self,
            _: &IssueRef,
            _: &str,
        ) -> Result<Comment, rupu_scm::ScmError> {
            unreachable!()
        }
        async fn create_issue(&self, _: &str, _: CreateIssue) -> Result<Issue, rupu_scm::ScmError> {
            unreachable!()
        }
        async fn update_issue_state(
            &self,
            _: &IssueRef,
            _: IssueState,
        ) -> Result<(), rupu_scm::ScmError> {
            unreachable!()
        }
    }
    fn request() -> ForgeRequest {
        ForgeRequest {
            action: ForgeAction::IssueCreate,
            target: ForgeTarget {
                provider: IssueTracker::Github,
                account: "work".into(),
                api_origin: "https://api.github.com".into(),
                project_ref: "org/repo".into(),
                number: None,
                ws_id: None,
            },
            expected: Default::default(),
            input: ForgeInput {
                title: Some("Review exact issue".into()),
                ..Default::default()
            },
        }
    }
    #[test]
    fn exact_account_and_origin_are_checked_without_authentication() {
        let mut registry = Registry::default();
        let work: Arc<dyn IssueConnector> = Arc::new(Adapter);
        let other: Arc<dyn IssueConnector> = Arc::new(Adapter);
        registry.insert_issue_account(AccountId::new("work"), Platform::Github, work.clone());
        registry.insert_issue_account(AccountId::new("other"), Platform::Github, other);
        let port = CpRepoLister {
            registry: Arc::new(registry),
        };
        assert!(Arc::ptr_eq(
            &resolve(&port, &request(), None).unwrap(),
            &work
        ));
        let mut r = request();
        r.target.api_origin = "https://other.example".into();
        assert_eq!(
            resolve(&port, &r, None).err().unwrap().code,
            "selection_changed"
        );
        r = request();
        r.target.account = "missing".into();
        assert_eq!(
            resolve(&port, &r, None).err().unwrap().code,
            "account_unavailable"
        );
    }
    #[test]
    fn issue_capabilities_do_not_imply_repository_write_authority() {
        let mut registry = Registry::default();
        registry.insert_issue_account(AccountId::new("work"), Platform::Github, Arc::new(Adapter));
        let port = CpRepoLister {
            registry: Arc::new(registry),
        };
        let rows = port.capabilities();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].actions, vec![ForgeAction::IssueCreate]);
        let mut r = request();
        r.action = ForgeAction::ReviewCreate;
        r.target.ws_id = Some("workspace".into());
        r.input.head = Some("topic".into());
        r.input.base = Some("main".into());
        assert!(resolve(&port, &r, None).is_err());
    }
    #[test]
    fn branch_creation_binds_checkout_authority_without_claiming_remote_base_is_checkout_head() {
        let owner = ForgeOwner {
            ws_id: "workspace".into(),
            canonical_root: "/fixture".into(),
            repo: rupu_scm::RepoRef {
                platform: Platform::Github,
                owner: "org".into(),
                repo: "repo".into(),
            },
            origin_host: "github.com".into(),
            origin_url_identity: "origin".into(),
            checkout_identity: "inode:1".into(),
            branch: "local-feature".into(),
            head: "a".repeat(40),
        };
        let mut req = request();
        req.action = ForgeAction::BranchCreate;
        req.target.ws_id = Some(owner.ws_id.clone());
        req.input = ForgeInput {
            head: Some("new".into()),
            base: Some("main".into()),
            ..Default::default()
        };
        let mut prepared = PreparedWrite {
            request: req,
            authority: json!({}),
            observed: json!({"base_sha":"b".repeat(40),"base_branch":"main","new_branch":"new"}),
            preconditions: json!({}),
            limitations: vec![],
            generation: "1".into(),
        };
        assert!(owner_authority(&mut prepared, None).is_err());
        owner_authority(&mut prepared, Some(&owner)).unwrap();
        assert_eq!(prepared.authority["checkout_head"], owner.head);
        assert_ne!(
            prepared.authority["checkout_head"],
            prepared.observed["base_sha"]
        );
        let prior = prepared.authority.clone();
        let mut changed = owner.clone();
        changed.checkout_identity = "inode:2".into();
        owner_authority(&mut prepared, Some(&changed)).unwrap();
        assert_ne!(prior, prepared.authority);
        prepared.request.action = ForgeAction::ReviewCreate;
        assert!(owner_authority(&mut prepared, Some(&owner)).is_err());
    }
}
