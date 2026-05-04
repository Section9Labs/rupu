//! GitHub workflow_dispatch — non-trait method exposed by Registry::github_extras().

use crate::error::ScmError;
use crate::types::{RepoRef, WorkflowDispatch};

use super::client::{classify_octocrab_error, GithubClient};

#[derive(Clone)]
pub struct GithubExtras {
    client: GithubClient,
}

impl GithubExtras {
    pub fn new(client: GithubClient) -> Self {
        Self { client }
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
            .with_retry(|| {
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
