//! GitHub workflow_dispatch — non-trait method exposed by Registry::github_extras().

use crate::error::ScmError;
use crate::types::{RepoRef, WorkflowDispatch};

use super::client::{classify_octocrab_error, GithubClient};

#[derive(Clone)]
pub struct GithubExtras {
    client: GithubClient,
}

impl GithubExtras {
    /// Connector lifetime plus nonsecret accepted credential generation.
    pub fn review_identity_epoch(&self) -> String {
        self.client.review_identity_epoch()
    }
    pub fn review_api_profile(&self) -> String {
        self.client.review_api_profile()
    }
    pub fn review_matches_origin(&self, origin: &str) -> bool {
        self.client.review_matches_origin(origin)
    }
    pub async fn review_get(
        &self,
        path: &str,
    ) -> Result<crate::review_transport::ReviewResponse, ScmError> {
        self.client.review_get(path).await
    }
    pub async fn review_get_bounded(
        &self,
        path: &str,
        max_bytes: usize,
    ) -> Result<crate::review_transport::ReviewResponse, ScmError> {
        self.client.review_get_bounded(path, max_bytes).await
    }

    pub async fn review_graphql(
        &self,
        query: &str,
        variables: serde_json::Value,
    ) -> Result<crate::review_transport::ReviewResponse, ScmError> {
        self.client.review_graphql(query, variables).await
    }
    pub async fn review_graphql_bounded(
        &self,
        query: &str,
        variables: serde_json::Value,
        max_bytes: usize,
    ) -> Result<crate::review_transport::ReviewResponse, ScmError> {
        self.client
            .review_graphql_bounded(query, variables, max_bytes)
            .await
    }

    pub fn new(client: GithubClient) -> Self {
        Self { client }
    }

    /// Compare origin authority without exposing a credential or configured URL.
    pub fn matches_web_host(&self, host: &str) -> bool {
        self.client
            .enterprise_host()
            .unwrap_or("github.com")
            .eq_ignore_ascii_case(host)
    }

    /// Diagnostic helper — exposes [`GithubClient::fetch_token_scopes`]
    /// without making the whole client `pub`. Used by `rupu repos list`
    /// to surface a missing-`repo`-scope warning when private repos
    /// are unexpectedly absent.
    pub async fn fetch_token_scopes(&self) -> Option<Vec<String>> {
        self.client.fetch_token_scopes().await
    }

    /// Exposes [`GithubClient::graphql_json`] without making the whole
    /// client `pub`. Used by `rupu webhook serve`'s GitHub Projects
    /// hydrator (Task 6): the hydrator resolves an `AccountId` via
    /// `Registry::github_extras_for` per inbound payload (the payload
    /// carries the org/owner), then runs its enrichment GraphQL queries
    /// against that account's already-built client — no client gets
    /// built per event, `Registry::discover` already built and cached
    /// one per configured account at daemon startup.
    pub async fn graphql_json(
        &self,
        query: &str,
        variables: serde_json::Value,
    ) -> Result<serde_json::Value, ScmError> {
        self.client.graphql_json(query, variables).await
    }

    pub async fn workflows_dispatch(
        &self,
        r: &RepoRef,
        w: WorkflowDispatch,
    ) -> Result<(), ScmError> {
        let _permit = self.client.permit().await;
        let inner = self.client.inner.clone();
        let owner = r.owner.clone();
        let repo = r.repo.clone();
        let workflow = w.workflow.clone();
        let ref_ = w.ref_.clone();
        let inputs = w.inputs.clone();
        self.client
            .with_retry_octocrab(|| {
                let inner = inner.clone();
                let owner = owner.clone();
                let repo = repo.clone();
                let workflow = workflow.clone();
                let ref_ = ref_.clone();
                let inputs = inputs.clone();
                async move {
                    let actions = inner.actions();
                    let mut req = actions.create_workflow_dispatch(&owner, &repo, &workflow, &ref_);
                    if inputs.is_object() {
                        req = req.inputs(inputs);
                    }
                    req.send().await.map_err(classify_octocrab_error)?;
                    Ok(())
                }
            })
            .await
    }
}
