//! GitLab pipeline_trigger — non-trait method exposed by Registry::gitlab_extras().

use crate::error::ScmError;
use crate::types::{PipelineTrigger, RepoRef};

use super::client::GitlabClient;

#[derive(Clone)]
pub struct GitlabExtras {
    client: GitlabClient,
}

impl GitlabExtras {
    pub fn new(client: GitlabClient) -> Self {
        Self { client }
    }

    pub async fn pipeline_trigger(&self, r: &RepoRef, p: PipelineTrigger) -> Result<(), ScmError> {
        let _permit = self.client.permit().await;
        let id = format!("{}/{}", r.owner, r.repo).replace('/', "%2F");
        let vars: Vec<serde_json::Value> = p
            .variables
            .iter()
            .map(|(k, v)| serde_json::json!({"key": k, "value": v}))
            .collect();
        let body = serde_json::json!({"ref": p.ref_, "variables": vars});
        self.client
            .write_json(
                reqwest::Method::POST,
                &format!("/projects/{id}/pipeline"),
                body,
            )
            .await?;
        Ok(())
    }
}
