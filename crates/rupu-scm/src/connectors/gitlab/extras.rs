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

    /// A public-host checkout must not resolve to an unrelated self-hosted account.
    pub fn matches_web_host(&self, host: &str) -> bool {
        self.client
            .self_managed_host()
            .as_deref()
            .unwrap_or("gitlab.com")
            .eq_ignore_ascii_case(host)
    }

    /// Explicit bounded read of this repository's current-branch merge requests.
    /// No dispatch, comments, credential changes or cross-project guessing.
    pub async fn branch_merge_requests(
        &self,
        r: &RepoRef,
        branch: &str,
    ) -> Result<(Vec<serde_json::Value>, bool), ScmError> {
        let _permit = self.client.permit().await;
        let id = url::form_urlencoded::byte_serialize(format!("{}/{}", r.owner, r.repo).as_bytes())
            .collect::<String>();
        let query = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("scope", "all")
            .append_pair("source_branch", branch)
            .append_pair("per_page", "6")
            .append_pair("order_by", "updated_at")
            .append_pair("sort", "desc")
            .finish();
        let page = self
            .client
            .get_json(&format!("/projects/{id}/merge_requests?{query}"))
            .await?;
        let rows = page.as_array().ok_or_else(|| ScmError::BadRequest {
            message: "Invalid merge request page".into(),
        })?;
        let truncated = rows.len() > 5;
        let mut result = Vec::new();
        for row in rows.iter().take(5) {
            // A same-named branch in someone else's fork is not this checkout.
            if row["source_branch"].as_str() != Some(branch)
                || row["source_project_id"].as_u64().is_none()
                || row["source_project_id"] != row["target_project_id"]
            {
                continue;
            }
            let iid = row["iid"].as_u64().ok_or_else(|| ScmError::BadRequest {
                message: "Missing merge request identity".into(),
            })?;
            let detail = self
                .client
                .get_json(&format!("/projects/{id}/merge_requests/{iid}"))
                .await?;
            if detail["iid"].as_u64() == Some(iid)
                && detail["source_branch"].as_str() == Some(branch)
                && detail["source_project_id"].as_u64().is_some()
                && detail["source_project_id"] == detail["target_project_id"]
            {
                result.push(detail);
            }
        }
        Ok((result, truncated))
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
