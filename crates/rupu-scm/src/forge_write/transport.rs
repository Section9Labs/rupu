use super::*;
use reqwest::{header::HeaderMap, Method};
use serde_json::json;
use std::sync::Arc;

const MAX_BYTES: usize = 2 * 1024 * 1024;

pub(crate) fn http(
    provider: &str,
    sink: Arc<dyn rupu_netflow::FlowSink>,
) -> anyhow::Result<reqwest_middleware::ClientWithMiddleware> {
    Ok(rupu_netflow::http::client_with(
        rupu_netflow::FlowCtx::system(rupu_netflow::Origin::Scm(provider.into())),
        reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .timeout(std::time::Duration::from_secs(30))
            .user_agent("rupu-operator-forge-write"),
        sink,
    )?)
}

/// Auth is frozen before the CP dispatch marker. It is never serialized or
/// included in Debug, error messages or receipts.
pub struct ForgeSession {
    pub(crate) provider: IssueTracker,
    pub(crate) origin: String,
    pub(crate) generation: String,
    http: reqwest_middleware::ClientWithMiddleware,
    auth: HeaderMap,
    current_generation: Arc<dyn Fn() -> String + Send + Sync>,
}
impl ForgeSession {
    pub(crate) fn new(
        provider: IssueTracker,
        origin: String,
        generation: String,
        http: reqwest_middleware::ClientWithMiddleware,
        auth: HeaderMap,
        current_generation: Arc<dyn Fn() -> String + Send + Sync>,
    ) -> Result<Self, ForgeError> {
        if crate::operator_issues::safe_origin(&origin).as_deref() != Some(origin.as_str()) {
            return Err(ForgeError::unsupported());
        }
        Ok(Self {
            provider,
            origin,
            generation,
            http,
            auth,
            current_generation,
        })
    }
    pub fn current(&self) -> bool {
        (self.current_generation)() == self.generation
    }
    fn url(&self, path: &str) -> Result<String, ForgeError> {
        if path.is_empty() && self.provider == IssueTracker::Linear {
            return Ok(self.origin.clone());
        }
        if !path.starts_with('/')
            || path.starts_with("//")
            || path.contains('#')
            || path.contains("..")
            || path.chars().any(char::is_control)
        {
            return Err(ForgeError::invalid());
        }
        Ok(format!("{}{path}", self.origin))
    }
    pub(crate) async fn get(&self, path: &str) -> Result<Value, ForgeError> {
        self.read(Method::GET, path, None).await
    }
    pub(crate) async fn graphql(&self, query: &str, variables: Value) -> Result<Value, ForgeError> {
        let value = self
            .read(
                Method::POST,
                "",
                Some(json!({"query":query,"variables":variables})),
            )
            .await?;
        if value
            .get("errors")
            .is_some_and(|e| !e.is_null() && e.as_array().is_none_or(|a| !a.is_empty()))
        {
            return Err(ForgeError::upstream());
        }
        value
            .get("data")
            .cloned()
            .filter(|v| v.is_object())
            .ok_or_else(ForgeError::upstream)
    }
    async fn read(
        &self,
        method: Method,
        path: &str,
        payload: Option<Value>,
    ) -> Result<Value, ForgeError> {
        if !self.current() {
            return Err(ForgeError::changed());
        }
        let mut req = self
            .http
            .request(method, self.url(path)?)
            .headers(self.auth.clone());
        if self.provider == IssueTracker::Github
            && path.starts_with("/repos/")
            && path.split('/').nth(4) == Some("actions") {
            req = req.header("X-GitHub-Api-Version", "2022-11-28");
        }
        if let Some(body) = payload {
            req = req.json(&body);
        }
        let response = req.send().await.map_err(|_| ForgeError::upstream())?;
        let status = response.status().as_u16();
        let (value, _) = bounded(response)
            .await
            .map_err(|_| ForgeError::upstream())?;
        if !self.current() {
            return Err(ForgeError::changed());
        }
        match status {
            200..=299 => Ok(value),
            401 | 403 => Err(ForgeError::fixed(
                "forbidden",
                "The selected account cannot perform this preparation read",
            )),
            404 => Err(ForgeError::fixed(
                "not_found",
                "The exact forge target was not found",
            )),
            429 => Err(ForgeError::fixed(
                "rate_limited",
                "The forge is rate limited",
            )),
            _ => Err(ForgeError::upstream()),
        }
    }
    pub(crate) fn mutation(
        self,
        method: Method,
        path: String,
        payload: Value,
        validation: Validation,
    ) -> Result<Box<dyn PreparedMutation>, ForgeError> {
        if !self.current() {
            return Err(ForgeError::changed());
        }
        let mut request = self
            .http
            .request(method, self.url(&path)?)
            .headers(self.auth.clone())
            .json(&payload);
        if validation.action == ForgeAction::WorkflowDispatch {
            request = request.header("X-GitHub-Api-Version", "2022-11-28");
        }
        let request = request.build()
            .map_err(|_| ForgeError::invalid())?;
        Ok(Box::new(Mutation {
            session: self,
            request,
            validation,
        }))
    }
}
async fn bounded(mut response: reqwest::Response) -> Result<(Value, bool), ()> {
    if response
        .content_length()
        .is_some_and(|n| n > MAX_BYTES as u64)
    {
        return Err(());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| ())? {
        if chunk.len() > MAX_BYTES.saturating_sub(bytes.len()) {
            return Err(());
        }
        bytes.extend_from_slice(&chunk);
    }
    if bytes.is_empty() {
        Ok((Value::Null, true))
    } else {
        serde_json::from_slice(&bytes).map(|value| (value, false)).map_err(|_| ())
    }
}
pub(crate) struct Validation {
    pub provider: IssueTracker,
    pub action: ForgeAction,
    pub origin: String,
    pub project: String,
    pub project_id: String,
    pub object_id: Option<String>,
    pub number: Option<String>,
    pub head: Option<String>,
    pub base: Option<String>,
    pub base_sha: Option<String>,
    pub state_id: Option<String>,
    pub desired_state: Option<String>,
    pub draft: Option<bool>,
    pub workflow_dispatch: Option<Value>,
}
struct Mutation {
    session: ForgeSession,
    request: reqwest::Request,
    validation: Validation,
}
#[async_trait::async_trait]
impl PreparedMutation for Mutation {
    fn is_current(&self) -> bool {
        self.session.current()
    }
    async fn dispatch(self: Box<Self>) -> DispatchResult {
        if !self.session.current() {
            return DispatchResult {
                outcome: DispatchOutcome::Unknown,
                reason: "Credential generation changed before dispatch; no request was sent".into(),
                result: json!({"request_sent":false}),
            };
        }
        // Exactly one API attempt. In particular no retry on 401/429/5xx.
        let response = match self.session.http.execute(self.request).await {
            Ok(v) => v,
            Err(_) => return DispatchResult::unknown(),
        };
        let status = response.status().as_u16();
        let (value, empty_body) = match bounded(response).await {
            Ok(v) => v,
            Err(_) => return DispatchResult::unknown(),
        };
        if matches!(status, 400 | 401 | 403 | 404 | 409 | 422 | 429) {
            // A contradictory partial mutation payload is never evidence of no effect.
            if value.get("id").is_some()
                || value.get("data").is_some_and(|v| !v.is_null())
                || (self.validation.action == ForgeAction::WorkflowDispatch
                    && ["workflow_run_id", "run_url", "html_url", "run_id", "check_suite_id", "workflow_runs"]
                        .iter().any(|key| value.get(key).is_some()))
                || (self.validation.action == ForgeAction::BranchCreate
                    && ["ref", "name", "commit", "object"]
                        .iter()
                        .any(|key| value.get(key).is_some()))
            {
                return DispatchResult::unknown();
            }

            return DispatchResult {
                outcome: DispatchOutcome::Rejected,
                reason: "The provider rejected the exact submitted action".into(),
                result: json!({"http_status":status}),
            };
        }
        if !(200..=299).contains(&status) {
            return DispatchResult::unknown();
        }
        if self.validation.action == ForgeAction::WorkflowDispatch && !empty_body {
            return DispatchResult::unknown();
        }
        match self.validation.accept(&value, status) {
            Some(result) => DispatchResult {
                outcome: DispatchOutcome::Accepted,
                reason: "The provider acknowledged the submitted action".into(),
                result,
            },
            None => DispatchResult::unknown(),
        }
    }
}
fn text(v: &Value) -> Option<String> {
    v.as_str()
        .filter(|s| !s.is_empty() && s.len() <= 512 && !s.chars().any(char::is_control))
        .map(str::to_string)
}
fn id(v: &Value) -> Option<String> {
    let value = text(v)
        .filter(|s| {
            s.len() <= 128
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
        })
        .or_else(|| v.as_u64().filter(|n| *n > 0).map(|n| n.to_string()))?;
    if value.bytes().all(|b| b.is_ascii_digit())
        && value
            .parse::<u64>()
            .ok()
            .filter(|n| *n > 0)
            .map(|n| n.to_string())
            .as_deref()
            != Some(value.as_str())
    {
        return None;
    }
    Some(value)
}

impl Validation {
    fn accept(&self, v: &Value, status: u16) -> Option<Value> {
        if self.action == ForgeAction::WorkflowDispatch {
            if self.provider != IssueTracker::Github || status != 204 || !v.is_null() { return None; }
            let mut result = self.workflow_dispatch.clone()?;
            result["effect"] = json!("provider_acknowledged");
            result["verification"] = json!("not_performed");
            return Some(result);
        }
        if self.action == ForgeAction::BranchCreate {
            // Both supported providers return 201 for creation. A 200 lookup is
            // not acknowledgement that this submitted operation created a branch.
            let contradictory_error = ["error", "errors"].iter().any(|key| {
                v.get(key).is_some_and(|error| match error {
                    Value::Null => false,
                    Value::String(s) => !s.is_empty(),
                    Value::Array(a) => !a.is_empty(),
                    Value::Object(o) => !o.is_empty(),
                    _ => true,
                })
            });
            if status != 201 || contradictory_error {
                return None;
            }
            let name = self.head.as_deref()?;
            let sha = self.base_sha.as_deref()?;
            match self.provider {
                IssueTracker::Github => {
                    let mut expected_url = url::Url::parse(&format!(
                        "{}/repos/{}/git/refs/heads/",
                        self.origin, self.project
                    ))
                    .ok()?;
                    expected_url
                        .path_segments_mut()
                        .ok()?
                        .pop_if_empty()
                        .extend(name.split('/'));
                    let returned_url = url::Url::parse(v["url"].as_str()?).ok()?;
                    if v["ref"].as_str() != Some(format!("refs/heads/{name}").as_str())
                        || returned_url != expected_url
                        || v["object"]["type"].as_str() != Some("commit")
                        || v["object"]["sha"].as_str() != Some(sha)
                    {
                        return None;
                    }
                }
                IssueTracker::Gitlab => {
                    if v["name"].as_str() != Some(name)
                        || v["commit"]["id"].as_str() != Some(sha)
                        || (v.get("project_id").is_some()
                            && id(&v["project_id"]).as_deref() != Some(self.project_id.as_str()))
                    {
                        return None;
                    }
                    // GitLab's branch DTO has no project_id. Ownership is the
                    // frozen authenticated request endpoint, proven in preflight.
                }
                _ => return None,
            }
            return Some(
                json!({"effect":"provider_acknowledged","verification":"returned_identity",
                "resource_id":format!("refs/heads/{name}"),"branch":name,"head_sha":sha,
                "provider_project_id":self.project_id}),
            );
        }
        let create = matches!(
            self.action,
            ForgeAction::ReviewCreate | ForgeAction::IssueCreate
        );
        let comment = matches!(
            self.action,
            ForgeAction::ReviewComment | ForgeAction::IssueComment
        );
        let mut object = v;
        if self.provider == IssueTracker::Linear {
            if v.get("errors")
                .is_some_and(|e| !e.is_null() && e.as_array().is_none_or(|a| !a.is_empty()))
            {
                return None;
            }
            let field = match self.action {
                ForgeAction::IssueCreate => "issueCreate",
                ForgeAction::IssueComment => "commentCreate",
                ForgeAction::IssueSetState => "issueUpdate",
                _ => return None,
            };
            let payload = &v["data"][field];
            if payload["success"].as_bool() != Some(true) {
                return None;
            }
            object = if comment {
                &payload["comment"]
            } else {
                &payload["issue"]
            };
            let parent = if comment { &object["issue"] } else { object };
            if id(&parent["team"]["id"]).as_deref() != Some(&self.project_id) {
                return None;
            }
            if !create && id(&parent["id"]) != self.object_id {
                return None;
            }
            if let Some(state) = &self.state_id {
                if id(&parent["state"]["id"]).as_ref() != Some(state) {
                    return None;
                }
            }
        } else if self.provider == IssueTracker::Jira {
            if self.action == ForgeAction::IssueSetState {
                return (status == 204).then(
                    || json!({"effect":"provider_acknowledged","verification":"not_performed"}),
                );
            }
            if comment {
                let cid = id(&object["id"])?;
                let iid = self.object_id.as_ref()?;
                if object["self"].as_str()
                    != Some(
                        format!("{}/rest/api/3/issue/{iid}/comment/{cid}", self.origin).as_str(),
                    )
                {
                    return None;
                }
            } else {
                let key = text(&object["key"])?;
                let suffix = key.strip_prefix(&format!("{}-", self.project))?;
                suffix.parse::<u64>().ok().filter(|n| *n > 0)?;
                let iid = id(&object["id"])?;
                if object["self"].as_str()
                    != Some(format!("{}/rest/api/3/issue/{iid}", self.origin).as_str())
                {
                    return None;
                }
            }
        } else if self.provider == IssueTracker::Github {
            let repo_url = format!("{}/repos/{}", self.origin, self.project);
            if comment {
                if object["issue_url"].as_str()
                    != Some(format!("{repo_url}/issues/{}", self.number.as_ref()?).as_str())
                {
                    return None;
                }
            } else if self.action == ForgeAction::ReviewCreate {
                if id(&object["base"]["repo"]["id"]).as_deref() != Some(&self.project_id)
                    || id(&object["head"]["repo"]["id"]).as_deref() != Some(&self.project_id)
                    || text(&object["head"]["ref"]) != self.head
                    || text(&object["base"]["ref"]) != self.base
                {
                    return None;
                }
                let n = id(&object["number"])?;
                if object["url"].as_str() != Some(format!("{repo_url}/pulls/{n}").as_str()) {
                    return None;
                }
            } else {
                if object["repository_url"].as_str() != Some(repo_url.as_str()) {
                    return None;
                }
                let n = id(&object["number"])?;
                if object["url"].as_str() != Some(format!("{repo_url}/issues/{n}").as_str())
                    || (!create && Some(n) != self.number)
                {
                    return None;
                }
            }
        } else {
            if id(&object["project_id"]).as_deref() != Some(&self.project_id) {
                return None;
            }
            if comment {
                if id(&object["noteable_id"]) != self.object_id
                    || id(&object["noteable_iid"]) != self.number
                {
                    return None;
                }
                let kind = if self.action.is_review() {
                    "MergeRequest"
                } else {
                    "Issue"
                };
                if object["noteable_type"].as_str() != Some(kind) {
                    return None;
                }
            } else if self.action == ForgeAction::ReviewCreate {
                if id(&object["source_project_id"]).as_deref() != Some(&self.project_id)
                    || id(&object["target_project_id"]).as_deref() != Some(&self.project_id)
                    || text(&object["source_branch"]) != self.head
                    || text(&object["target_branch"]) != self.base
                {
                    return None;
                }
            } else if !create && id(&object["iid"]) != self.number {
                return None;
            }
        }
        if self.action == ForgeAction::IssueSetState
            && matches!(self.provider, IssueTracker::Github | IssueTracker::Gitlab)
            && id(&object["id"]) != self.object_id
        {
            return None;
        }
        if create && self.provider == IssueTracker::Gitlab {
            id(&object["iid"])?;
        }
        if self.provider == IssueTracker::Linear && !comment {
            let identifier = object["identifier"].as_str()?;
            let suffix = identifier.strip_prefix(&format!("{}-", self.project))?;
            let n = suffix.parse::<u64>().ok().filter(|n| *n > 0)?;
            if n.to_string() != suffix || (!create && self.number.as_deref() != Some(suffix)) {
                return None;
            }
        }
        if self.action == ForgeAction::IssueSetState
            && matches!(self.provider, IssueTracker::Github | IssueTracker::Gitlab)
        {
            let wanted = if self.provider == IssueTracker::Gitlab
                && self.desired_state.as_deref() == Some("open")
            {
                "opened"
            } else {
                self.desired_state.as_deref()?
            };
            if object["state"].as_str() != Some(wanted) {
                return None;
            }
        }
        if let Some(draft) = self.draft {
            if object
                .get("draft")
                .or_else(|| object.get("work_in_progress"))
                .and_then(Value::as_bool)
                != Some(draft)
            {
                return None;
            }
        }
        let resource_id = id(&object["id"])?;
        let number = object
            .get("number")
            .or_else(|| object.get("iid"))
            .and_then(id);
        Some(
            json!({"effect":"provider_acknowledged","verification":"returned_identity","resource_id":resource_id,"number":number,"key":object.get("key").and_then(text),"head_sha":object.get("sha").and_then(text).or_else(||object.get("head").and_then(|h|h.get("sha")).and_then(text))}),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::prelude::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    fn session(server: &MockServer, epoch: Arc<AtomicUsize>) -> ForgeSession {
        crate::install_default_crypto_provider();
        let mut auth = HeaderMap::new();
        auth.insert(
            reqwest::header::AUTHORIZATION,
            reqwest::header::HeaderValue::from_static("Bearer fixture"),
        );
        ForgeSession::new(
            IssueTracker::Github,
            server.base_url(),
            "1".into(),
            http("github", Arc::new(rupu_netflow::NullSink)).unwrap(),
            auth,
            Arc::new(move || epoch.load(Ordering::SeqCst).to_string()),
        )
        .unwrap()
    }
    fn validation(origin: String) -> Validation {
        Validation {
            provider: IssueTracker::Github,
            action: ForgeAction::IssueComment,
            origin,
            project: "org/repo".into(),
            project_id: "1".into(),
            object_id: Some("7".into()),
            number: Some("7".into()),
            head: None,
            base: None,
            base_sha: None,
            state_id: None,
            desired_state: None,
            draft: None,
            workflow_dispatch: None,
        }
    }
    #[tokio::test]
    async fn one_dispatch_on_refusal_server_failure_or_oversized_reply() {
        for status in [401u16, 429, 500, 200] {
            let server = MockServer::start_async().await;
            let row = server.mock(|when, then| {
                when.method(POST)
                    .path("/repos/org/repo/issues/7/comments")
                    .header("authorization", "Bearer fixture");
                then.status(status).body(if status == 200 {
                    "x".repeat(MAX_BYTES + 1)
                } else {
                    "{}".into()
                });
            });
            let mutation = session(&server, Arc::new(AtomicUsize::new(1)))
                .mutation(
                    Method::POST,
                    "/repos/org/repo/issues/7/comments".into(),
                    json!({"body":"exact text"}),
                    validation(server.base_url()),
                )
                .unwrap();
            let result = mutation.dispatch().await;
            assert_eq!(
                result.outcome,
                if status == 401 || status == 429 {
                    DispatchOutcome::Rejected
                } else {
                    DispatchOutcome::Unknown
                }
            );
            row.assert_hits(1);
        }
    }
    #[tokio::test]
    async fn redirect_never_forwards_frozen_credentials() {
        let server = MockServer::start_async().await;
        let other = MockServer::start_async().await;
        let target = other.mock(|when, then| {
            when.method(POST);
            then.status(200).json_body(json!({"id":99}));
        });
        let initial = server.mock(|when, then| {
            when.method(POST);
            then.status(307).header("Location", other.url("/collect"));
        });
        let mutation = session(&server, Arc::new(AtomicUsize::new(1)))
            .mutation(
                Method::POST,
                "/repos/org/repo/issues/7/comments".into(),
                json!({"body":"x"}),
                validation(server.base_url()),
            )
            .unwrap();
        assert_eq!(mutation.dispatch().await.outcome, DispatchOutcome::Unknown);
        initial.assert_hits(1);
        target.assert_hits(0);
    }
    #[tokio::test]
    async fn changed_credential_generation_prevents_dispatch() {
        let server = MockServer::start_async().await;
        let target = server.mock(|when, then| {
            when.method(POST);
            then.status(200);
        });
        let epoch = Arc::new(AtomicUsize::new(1));
        let mutation = session(&server, epoch.clone())
            .mutation(
                Method::POST,
                "/repos/org/repo/issues/7/comments".into(),
                json!({"body":"x"}),
                validation(server.base_url()),
            )
            .unwrap();
        epoch.store(2, Ordering::SeqCst);
        let result = mutation.dispatch().await;
        assert_eq!(result.outcome, DispatchOutcome::Unknown);
        assert_eq!(result.result["request_sent"], false);
        target.assert_hits(0);
    }
    #[test]
    fn acknowledgement_requires_exact_parent_identity() {
        let v = validation("https://api.github.com".into());
        assert!(v
            .accept(
                &json!({"id":99,"issue_url":"https://api.github.com/repos/other/repo/issues/7"}),
                201
            )
            .is_none());
        assert!(v
            .accept(
                &json!({"id":99,"issue_url":"https://api.github.com/repos/org/repo/issues/7"}),
                201
            )
            .is_some());
    }
    #[test]
    fn gitlab_note_acknowledgement_binds_project_and_parent() {
        let mut v = validation("https://gitlab.example/api/v4".into());
        v.provider = IssueTracker::Gitlab;
        let good = json!({"id":99,"project_id":1,"noteable_id":7,"noteable_iid":7,"noteable_type":"Issue"});
        assert!(v.accept(&good, 201).is_some());
        let mut bad = good.clone();
        bad["noteable_id"] = json!(8);
        assert!(v.accept(&bad, 201).is_none());
        let mut bad = good;
        bad["project_id"] = json!(2);
        assert!(v.accept(&bad, 201).is_none());
    }
    #[test]
    fn jira_and_linear_acknowledgements_preserve_identity_and_partial_unknown() {
        let mut jira = validation("https://jira.example".into());
        jira.provider = IssueTracker::Jira;
        assert!(jira
            .accept(
                &json!({"id":"99","self":"https://jira.example/rest/api/3/issue/7/comment/99"}),
                201
            )
            .is_some());
        assert!(jira
            .accept(
                &json!({"id":"99","self":"https://jira.example/rest/api/3/issue/8/comment/99"}),
                201
            )
            .is_none());
        jira.action = ForgeAction::IssueSetState;
        assert_eq!(
            jira.accept(&Value::Null, 204).unwrap()["verification"],
            "not_performed"
        );
        let mut linear = validation("https://api.linear.app/graphql".into());
        linear.provider = IssueTracker::Linear;
        let good = json!({"data":{"commentCreate":{"success":true,"comment":{"id":"comment-99","issue":{"id":"7","team":{"id":"1"}}}}}});
        assert!(linear.accept(&good, 200).is_some());
        let mut bad = good;
        bad["errors"] = json!([{"message":"partial"}]);
        assert!(linear.accept(&bad, 200).is_none());
    }
    fn branch_request(server: &MockServer, provider: IssueTracker) -> ForgeRequest {
        ForgeRequest {
            action: ForgeAction::BranchCreate,
            target: ForgeTarget {
                provider,
                account: "work".into(),
                api_origin: server.base_url(),
                project_ref: "org/repo".into(),
                number: None,
                ws_id: Some("workspace".into()),
            },
            expected: Default::default(),
            input: ForgeInput {
                head: Some("feature/new".into()),
                base: Some("main".into()),
                ..Default::default()
            },
        }
    }
    fn branch_session(server: &MockServer, provider: IssueTracker) -> ForgeSession {
        let mut value = session(server, Arc::new(AtomicUsize::new(1)));
        value.provider = provider;
        value
    }
    fn branch_repo(server: &MockServer, provider: IssueTracker) -> httpmock::Mock<'_> {
        server.mock(|when, then| {
            if provider == IssueTracker::Github {
                when.method(GET).path("/repos/org/repo");
                then.status(200).json_body(
                    json!({"id":1,"full_name":"org/repo","url":server.url("/repos/org/repo")}),
                );
            } else {
                when.method(GET).path("/projects/org%2Frepo");
                then.status(200)
                    .json_body(json!({"id":1,"path_with_namespace":"org/repo"}));
            }
        })
    }
    fn branch_base<'a>(
        server: &'a MockServer,
        provider: IssueTracker,
        sha: &str,
    ) -> httpmock::Mock<'a> {
        server.mock(|when, then| {
            when.method(GET).path(if provider == IssueTracker::Github {
                "/repos/org/repo/branches/main"
            } else {
                "/projects/1/repository/branches/main"
            });
            then.status(200)
                .json_body(json!({"name":"main","commit":{"sha":sha,"id":sha}}));
        })
    }
    fn branch_absent(
        server: &MockServer,
        provider: IssueTracker,
        status: u16,
    ) -> httpmock::Mock<'_> {
        server.mock(|when, then| {
            when.method(GET).path(if provider == IssueTracker::Github {
                "/repos/org/repo/branches/feature%2Fnew"
            } else {
                "/projects/1/repository/branches/feature%2Fnew"
            });
            then.status(status).json_body(json!({"message":"fixture"}));
        })
    }
    #[tokio::test]
    async fn branch_creation_pins_commit_and_acknowledges_exact_name_for_both_providers() {
        for provider in [IssueTracker::Github, IssueTracker::Gitlab] {
            let server = MockServer::start_async().await;
            let sha = "a".repeat(40);
            let _repo = branch_repo(&server, provider);
            let base = branch_base(&server, provider, &sha);
            let absent = branch_absent(&server, provider, 404);
            let post = server.mock(|when, then| {
                if provider == IssueTracker::Github {
                    when.method(POST).path("/repos/org/repo/git/refs").json_body(json!({"ref":"refs/heads/feature/new","sha":sha}));
                    then.status(201).json_body(json!({"ref":"refs/heads/feature/new","url":server.url("/repos/org/repo/git/refs/heads/feature/new"),"object":{"type":"commit","sha":sha}}));
                } else {
                    when.method(POST).path("/projects/1/repository/branches").json_body(json!({"branch":"feature/new","ref":sha}));
                    then.status(201).json_body(json!({"name":"feature/new","commit":{"id":sha}}));
                }
            });
            let prepared = branch_session(&server, provider)
                .prepare(&branch_request(&server, provider))
                .await
                .unwrap();
            assert_eq!(prepared.observed["base_sha"], sha);
            assert_eq!(prepared.observed["new_branch_absent"], true);
            let mutation = branch_session(&server, provider)
                .preflight(&prepared)
                .await
                .unwrap();
            let result = mutation.dispatch().await;
            assert_eq!(result.outcome, DispatchOutcome::Accepted);
            assert_eq!(result.result["resource_id"], "refs/heads/feature/new");
            assert_eq!(result.result["head_sha"], sha);
            base.assert_hits(2);
            absent.assert_hits(2);
            post.assert_hits(1);
        }
    }
    #[tokio::test]
    async fn branch_base_movement_and_new_duplicate_refuse_before_dispatch() {
        for provider in [IssueTracker::Github, IssueTracker::Gitlab] {
            let server = MockServer::start_async().await;
            let _repo = branch_repo(&server, provider);
            let mut base = branch_base(&server, provider, &"a".repeat(40));
            let mut absent = branch_absent(&server, provider, 404);
            let post = server.mock(|when, then| {
                when.method(POST);
                then.status(201);
            });
            let prepared = branch_session(&server, provider)
                .prepare(&branch_request(&server, provider))
                .await
                .unwrap();
            base.delete();
            let _changed = branch_base(&server, provider, &"b".repeat(40));
            assert_eq!(
                branch_session(&server, provider)
                    .preflight(&prepared)
                    .await
                    .err()
                    .unwrap()
                    .code,
                "selection_changed"
            );
            absent.delete();
            let _exists = branch_absent(&server, provider, 200);
            assert_eq!(
                branch_session(&server, provider)
                    .prepare(&branch_request(&server, provider))
                    .await
                    .unwrap_err()
                    .code,
                "selection_changed"
            );
            post.assert_hits(0);
        }
    }
    #[tokio::test]
    async fn branch_absence_requires_404_and_provider_identity_is_exact() {
        let server = MockServer::start_async().await;
        let mut repo = branch_repo(&server, IssueTracker::Github);
        let _base = branch_base(&server, IssueTracker::Github, &"a".repeat(40));
        let _forbidden = branch_absent(&server, IssueTracker::Github, 403);
        let req = branch_request(&server, IssueTracker::Github);
        assert!(branch_session(&server, IssueTracker::Github)
            .prepare(&req)
            .await
            .is_err());
        repo.delete();
        let _other = server.mock(|when, then| {
            when.method(GET).path("/repos/org/repo");
            then.status(200).json_body(
                json!({"id":2,"full_name":"other/repo","url":server.url("/repos/other/repo")}),
            );
        });
        assert_eq!(
            branch_session(&server, IssueTracker::Github)
                .prepare(&req)
                .await
                .unwrap_err()
                .code,
            "selection_changed"
        );
        assert_eq!(
            branch_session(&server, IssueTracker::Gitlab)
                .prepare(&req)
                .await
                .unwrap_err()
                .code,
            "selection_changed"
        );
    }
    #[test]
    fn branch_receipt_requires_exact_ref_commit_and_creation_status() {
        let mut check = validation("https://api.github.com".into());
        check.action = ForgeAction::BranchCreate;
        check.head = Some("feature/new".into());
        check.base_sha = Some("a".repeat(40));
        let good = json!({"ref":"refs/heads/feature/new","url":"https://api.github.com/repos/org/repo/git/refs/heads/feature/new","object":{"type":"commit","sha":"a".repeat(40)}});
        assert!(check.accept(&good, 201).is_some());
        assert!(check.accept(&good, 200).is_none());
        for key in ["error", "errors"] {
            let mut contradiction = good.clone();
            contradiction[key] = json!(["partial failure"]);
            assert!(check.accept(&contradiction, 201).is_none());
        }
        for (key, value) in [
            ("ref", json!("refs/heads/other")),
            (
                "url",
                json!("https://api.github.com/repos/other/repo/git/refs/heads/feature/new"),
            ),
            ("object", json!({"type":"commit","sha":"b".repeat(40)})),
        ] {
            let mut bad = good.clone();
            bad[key] = value;
            assert!(check.accept(&bad, 201).is_none());
        }
        check.provider = IssueTracker::Gitlab;
        assert!(check
            .accept(
                &json!({"name":"feature/new","commit":{"id":"a".repeat(40)},"project_id":2}),
                201
            )
            .is_none());
        assert!(check
            .accept(
                &json!({"name":"feature/new","commit":{"id":"a".repeat(40)}}),
                201
            )
            .is_some());
        assert!(check
            .accept(
                &json!({"name":"feature/new","commit":{"id":"b".repeat(40)}}),
                201
            )
            .is_none());
    }
    #[tokio::test]
    async fn branch_contradictory_rejection_and_mismatched_success_are_unknown_once() {
        for status in [409u16, 201, 500] {
            let server = MockServer::start_async().await;
            let post = server.mock(|when, then| {
                when.method(POST);
                then.status(status)
                    .json_body(json!({"ref":"refs/heads/other","object":{"sha":"b".repeat(40)}}));
            });
            let mut check = validation(server.base_url());
            check.action = ForgeAction::BranchCreate;
            check.head = Some("feature/new".into());
            check.base_sha = Some("a".repeat(40));
            let result = session(&server, Arc::new(AtomicUsize::new(1)))
                .mutation(
                    Method::POST,
                    "/repos/org/repo/git/refs".into(),
                    json!({"ref":"refs/heads/feature/new","sha":"a".repeat(40)}),
                    check,
                )
                .unwrap()
                .dispatch()
                .await;
            assert_eq!(result.outcome, DispatchOutcome::Unknown);
            post.assert_hits(1);
        }
    }
    #[test]
    fn workflow_dispatch_accepts_only_empty_204_without_run_identity() {
        let mut check = validation("https://api.github.com".into());
        check.action = ForgeAction::WorkflowDispatch;
        check.workflow_dispatch = Some(json!({"workflow_id":"42","ref_name":"main","ref_kind":"branch","ref_sha":"a".repeat(40),"provider_project_id":"1"}));
        let result = check.accept(&Value::Null, 204).unwrap();
        assert_eq!(result["verification"], "not_performed");
        assert!(result.get("run_id").is_none() && result.get("resource_id").is_none());
        assert!(check.accept(&json!({"id":123}), 200).is_none());
        assert!(check.accept(&json!({"error":"contradiction"}), 204).is_none());
        assert!(check.accept(&Value::Null, 202).is_none());
        check.provider = IssueTracker::Gitlab;
        assert!(check.accept(&Value::Null, 204).is_none());
    }

    #[tokio::test]
    async fn bounded_reader_distinguishes_received_empty_bytes_from_json_null() {
        crate::install_default_crypto_provider();
        let server = MockServer::start_async().await;
        for (path, body, expected_empty) in [("/empty", "", true), ("/null", "null", false)] {
            let read = server.mock(|when, then| { when.method(GET).path(path); then.status(200).body(body); });
            let response = http("github", Arc::new(rupu_netflow::NullSink)).unwrap().get(server.url(path)).send().await.unwrap();
            let (value, empty) = bounded(response).await.unwrap();
            assert!(value.is_null()); assert_eq!(empty, expected_empty); read.assert_hits(1);
        }
    }

}
