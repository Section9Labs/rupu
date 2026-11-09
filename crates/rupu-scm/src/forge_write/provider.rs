use super::{transport::Validation, *};
use reqwest::Method;
use serde_json::json;
use sha2::{Digest, Sha256};
#[path = "workflow_dispatch.rs"]
mod workflow_dispatch;

fn clean(s: &str, n: usize) -> bool {
    !s.is_empty() && s.len() <= n && !s.chars().any(char::is_control)
}
fn id(v: &Value) -> Result<String, ForgeError> {
    let value = v
        .as_str()
        .filter(|s| {
            clean(s, 128)
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
        })
        .map(str::to_string)
        .or_else(|| v.as_u64().filter(|n| *n > 0).map(|n| n.to_string()))
        .ok_or_else(ForgeError::upstream)?;
    if value.bytes().all(|b| b.is_ascii_digit())
        && value
            .parse::<u64>()
            .ok()
            .filter(|n| *n > 0)
            .map(|n| n.to_string())
            .as_deref()
            != Some(value.as_str())
    {
        return Err(ForgeError::upstream());
    }
    Ok(value)
}

fn string(v: &Value) -> Result<String, ForgeError> {
    v.as_str()
        .filter(|s| clean(s, 512))
        .map(str::to_string)
        .ok_or_else(ForgeError::upstream)
}
fn enc(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}
fn hash(v: &Value) -> Result<String, ForgeError> {
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(v).map_err(|_| ForgeError::upstream())?)
    ))
}
fn unchanged(actual: &str, expected: &str) -> Result<(), ForgeError> {
    if actual == expected {
        Ok(())
    } else {
        Err(ForgeError::changed())
    }
}
fn number(req: &ForgeRequest) -> Result<String, ForgeError> {
    req.target.number.clone().ok_or_else(ForgeError::invalid)
}
fn body(req: &ForgeRequest) -> String {
    req.input.body.clone().unwrap_or_default()
}
fn title(req: &ForgeRequest) -> Result<String, ForgeError> {
    req.input.title.clone().ok_or_else(ForgeError::invalid)
}
fn branch(s: &Option<String>) -> Result<String, ForgeError> {
    s.clone()
        .filter(|s| {
            clean(s, 255)
                && !s.starts_with('-')
                && !s.contains("..")
                && !s.contains(['~', '^', ':', '?', '*', '[', '\\', ' '])
        })
        .ok_or_else(ForgeError::invalid)
}
// Branch creation accepts branch names, never arbitrary revision expressions.
fn create_branch_name(s: &Option<String>) -> Result<String, ForgeError> {
    let name = branch(s)?;
    if name == "HEAD"
        || name == "@"
        || name.starts_with("refs/")
        || name.ends_with('.')
        || name.contains("@{")
        || name.chars().any(char::is_whitespace)
        || name
            .split('/')
            .any(|part| part.is_empty() || part.starts_with('.') || part.ends_with(".lock"))
    {
        return Err(ForgeError::invalid());
    }
    Ok(name)
}
fn commit_sha(v: &Value) -> Result<String, ForgeError> {
    let sha = string(v)?;
    if !matches!(sha.len(), 40 | 64) || !sha.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(ForgeError::upstream());
    }
    Ok(sha)
}
fn state(req: &ForgeRequest) -> Result<&str, ForgeError> {
    req.input
        .state
        .as_deref()
        .filter(|s| matches!(*s, "open" | "closed"))
        .ok_or_else(ForgeError::invalid)
}
fn rows<'a>(v: &'a Value, key: &str) -> Result<&'a Vec<Value>, ForgeError> {
    v.get(key)
        .unwrap_or(v)
        .as_array()
        .filter(|a| a.len() <= 100)
        .ok_or_else(ForgeError::upstream)
}
fn choices(req: &ForgeRequest, values: Vec<ForgeChoice>) -> Result<ForgeChoice, ForgeError> {
    if values.is_empty() {
        return Err(ForgeError::unsupported());
    }
    if values
        .iter()
        .map(|v| v.id.as_str())
        .collect::<std::collections::BTreeSet<_>>()
        .len()
        != values.len()
    {
        return Err(ForgeError::upstream());
    }
    if let Some(chosen) = &req.input.choice_id {
        return values
            .into_iter()
            .find(|v| &v.id == chosen)
            .ok_or_else(ForgeError::changed);
    }
    Err(ForgeError::choice(values))
}
pub fn validate_request(req: &ForgeRequest) -> Result<(), ForgeError> {
    let target = &req.target;
    let input = &req.input;
    if !clean(&target.account, 128)
        || !clean(&target.project_ref, 512)
        || crate::operator_issues::safe_origin(&target.api_origin).as_deref()
            != Some(target.api_origin.as_str())
        || target.api_origin.len() > 2048
    {
        return Err(ForgeError::invalid());
    }
    if req.action.is_repository() {
        if !matches!(target.provider, IssueTracker::Github | IssueTracker::Gitlab)
            || target.ws_id.as_deref().is_none_or(|v| !clean(v, 128))
        {
            return Err(ForgeError::invalid());
        }
    } else if target.ws_id.is_some() {
        return Err(ForgeError::invalid());
    }
    if req.action.needs_existing_number() {
        let n = target.number.as_deref().ok_or_else(ForgeError::invalid)?;
        if n.parse::<u64>()
            .ok()
            .filter(|n| *n > 0)
            .map(|v| v.to_string())
            .as_deref()
            != Some(n)
        {
            return Err(ForgeError::invalid());
        }
    } else if target.number.is_some() {
        return Err(ForgeError::invalid());
    }
    if req.action != ForgeAction::WorkflowDispatch
        && (input.workflow.is_some() || input.ref_name.is_some() || input.ref_kind.is_some() || !input.workflow_inputs.is_empty())
    { return Err(ForgeError::invalid()); }
    let allowed: &[&str] = match req.action {
        ForgeAction::WorkflowDispatch => &["provider_project_id", "workflow_id", "ref_sha"],
        ForgeAction::ReviewComment => &[
            "provider_project_id",
            "object_id",
            "head_sha",
            "base_sha",
            "state",
        ],
        ForgeAction::ReviewCreate => &["provider_project_id", "head_sha", "base_sha"],
        ForgeAction::BranchCreate => &["provider_project_id", "base_sha"],
        ForgeAction::IssueComment | ForgeAction::IssueSetState => {
            &["version", "provider_project_id", "object_id", "state"]
        }
        ForgeAction::IssueCreate => &["provider_project_id"],
    };
    if req.expected.len() > 8
        || req
            .expected
            .iter()
            .any(|(k, v)| !allowed.contains(&k.as_str()) || !clean(v, 512))
    {
        return Err(ForgeError::invalid());
    }
    if input
        .body
        .as_ref()
        .is_some_and(|b| b.len() > 64 * 1024 || b.contains('\0'))
        || input.title.as_ref().is_some_and(|s| !clean(s, 512))
        || input.labels.len() > 32
        || input.labels.iter().any(|s| !clean(s, 256))
        || input
            .labels
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            != input.labels.len()
        || input.choice_id.as_ref().is_some_and(|s| !clean(s, 128))
    {
        return Err(ForgeError::invalid());
    }
    // GitLab interprets slash-prefixed body lines as additional mutations.
    // Keep operator comments/creation within the explicitly reviewed action.
    if target.provider == IssueTracker::Gitlab
        && input.body.as_deref().is_some_and(|body| {
            body.split([
                '\n', '\r', '\u{0085}', '\u{2028}', '\u{2029}', '\u{000b}', '\u{000c}',
            ])
            .any(|line| line.trim_start().starts_with('/'))
        })
    {
        return Err(ForgeError::fixed(
            "unsupported_quick_action",
            "GitLab quick-action lines are not supported in operator comments or descriptions",
        ));
    }
    match req.action {
        ForgeAction::WorkflowDispatch => workflow_dispatch::validate(req)?,
        ForgeAction::BranchCreate => {
            create_branch_name(&input.head)?;
            create_branch_name(&input.base)?;
            if input.head == input.base
                || input.title.is_some()
                || input.body.is_some()
                || !input.labels.is_empty()
                || input.draft.is_some()
                || input.state.is_some()
                || input.choice_id.is_some()
            {
                return Err(ForgeError::invalid());
            }
        }
        ForgeAction::ReviewComment | ForgeAction::IssueComment => {
            if input.body.as_deref().is_none_or(str::is_empty)
                || input.title.is_some()
                || !input.labels.is_empty()
                || input.head.is_some()
                || input.base.is_some()
                || input.draft.is_some()
                || input.state.is_some()
                || input.choice_id.is_some()
            {
                return Err(ForgeError::invalid());
            }
        }
        ForgeAction::ReviewCreate => {
            title(req)?;
            branch(&input.head)?;
            branch(&input.base)?;
            if input.head == input.base
                || !input.labels.is_empty()
                || input.state.is_some()
                || input.choice_id.is_some()
            {
                return Err(ForgeError::invalid());
            }
        }
        ForgeAction::IssueCreate => {
            title(req)?;
            if input.head.is_some()
                || input.base.is_some()
                || input.draft.is_some()
                || input.state.is_some()
            {
                return Err(ForgeError::invalid());
            }
        }
        ForgeAction::IssueSetState => {
            state(req)?;
            if input.title.is_some()
                || input.body.is_some()
                || !input.labels.is_empty()
                || input.head.is_some()
                || input.base.is_some()
                || input.draft.is_some()
            {
                return Err(ForgeError::invalid());
            }
        }
    }
    if matches!(
        req.action,
        ForgeAction::IssueComment | ForgeAction::IssueSetState
    ) && !req.expected.contains_key("version")
    {
        return Err(ForgeError::invalid());
    }
    if req.action == ForgeAction::ReviewComment && !req.expected.contains_key("head_sha") {
        return Err(ForgeError::invalid());
    }
    if !matches!(target.provider, IssueTracker::Jira | IssueTracker::Linear)
        && input.choice_id.is_some()
    {
        return Err(ForgeError::invalid());
    }
    Ok(())
}
struct Plan {
    observed: Value,
    method: Method,
    path: String,
    payload: Value,
    validation: Validation,
}
impl ForgeSession {
    pub async fn prepare(&self, req: &ForgeRequest) -> Result<PreparedWrite, ForgeError> {
        validate_request(req)?;
        if req.target.provider != self.provider
            || req.target.api_origin != self.origin
            || !self.current()
        {
            return Err(ForgeError::changed());
        }
        let plan = self.plan(req).await?;
        self.prepared(req, &plan)
    }
    pub async fn preflight(
        self,
        prepared: &PreparedWrite,
    ) -> Result<Box<dyn PreparedMutation>, ForgeError> {
        validate_request(&prepared.request)?;
        if prepared.generation != self.generation
            || prepared.request.target.api_origin != self.origin
            || prepared.request.target.provider != self.provider
        {
            return Err(ForgeError::changed());
        }
        let plan = self.plan(&prepared.request).await?;
        let fresh = self.prepared(&prepared.request, &plan)?;
        // CLI supplies repository ownership fields outside provider preparation.
        if fresh.observed != prepared.observed
            || fresh.preconditions != prepared.preconditions
            || fresh.generation != prepared.generation
        {
            return Err(ForgeError::changed());
        }
        self.mutation(plan.method, plan.path, plan.payload, plan.validation)
    }
    fn prepared(&self, req: &ForgeRequest, plan: &Plan) -> Result<PreparedWrite, ForgeError> {
        if !self.current() {
            return Err(ForgeError::changed());
        }
        for (key, value) in &req.expected {
            if key == "version" {
                continue;
            } // G1 typed issue version is bracket-checked by owning CLI.
            let actual = plan
                .observed
                .get(key)
                .and_then(Value::as_str)
                .ok_or_else(ForgeError::changed)?;
            unchanged(actual, value)?;
        }
        Ok(PreparedWrite{request:req.clone(),authority:json!({"host_id":"local","provider":self.provider,"account":req.target.account,"api_origin":self.origin,"project_ref":req.target.project_ref,"provider_project_id":plan.observed["provider_project_id"],"provider_object_id":plan.observed.get("object_id")}),observed:plan.observed.clone(),preconditions:json!({"snapshot":hash(&plan.observed)?,"atomic":false,"provider_cas":false}),limitations:if req.action == ForgeAction::WorkflowDispatch { vec!["The branch or tag and workflow can change after the final check; dispatch uses the ref name, not a commit compare-and-swap.".into(),"An annotated tag records its tag-object SHA; it is not a peeled commit SHA.".into(),"Only provider acknowledgement is returned. No workflow run ID, completion or successful execution is established.".into(),"A timeout or uncertain acknowledgement is not safe to retry automatically.".into()] } else { vec!["The forge can change after the final check; this action has no provider compare-and-swap guarantee.".into(),"A timeout or uncertain acknowledgement is not safe to retry automatically.".into()] },generation:self.generation.clone()})
    }
    async fn plan(&self, req: &ForgeRequest) -> Result<Plan, ForgeError> {
        match self.provider {
            IssueTracker::Github => self.github(req).await,
            IssueTracker::Gitlab => self.gitlab(req).await,
            IssueTracker::Jira => self.jira(req).await,
            IssueTracker::Linear => self.linear(req).await,
        }
    }
    fn validation(&self, req: &ForgeRequest, pid: String, oid: Option<String>) -> Validation {
        Validation {
            provider: self.provider,
            action: req.action,
            origin: self.origin.clone(),
            project: req.target.project_ref.clone(),
            project_id: pid,
            object_id: oid,
            number: req.target.number.clone(),
            head: req.input.head.clone(),
            base: req.input.base.clone(),
            base_sha: None,
            state_id: None,
            desired_state: req.input.state.clone(),
            workflow_dispatch: None,
            draft: (req.action == ForgeAction::ReviewCreate)
                .then_some(req.input.draft.unwrap_or(false)),
        }
    }
    async fn github(&self, req: &ForgeRequest) -> Result<Plan, ForgeError> {
        crate::operator_issues::validate_project(&crate::IssueRef {
            tracker: self.provider,
            project: req.target.project_ref.clone(),
            number: 1,
        })
        .map_err(|_| ForgeError::invalid())?;
        let project = &req.target.project_ref;
        let root = format!("/repos/{project}");
        let repo = self.get(&root).await?;
        unchanged(&string(&repo["full_name"])?, project)?;
        unchanged(&string(&repo["url"])?, &format!("{}{root}", self.origin))?;
        let pid = id(&repo["id"])?;
        if req.action == ForgeAction::WorkflowDispatch {
            return self.github_workflow_dispatch(req, &root, pid).await;
        }
        let mut observed = json!({"provider_project_id":pid});
        let mut validation = self.validation(req, pid.clone(), None);
        let (method, path, payload) = match req.action {
            ForgeAction::BranchCreate => {
                let name = create_branch_name(&req.input.head)?;
                let base = create_branch_name(&req.input.base)?;
                let b = self.get(&format!("{root}/branches/{}", enc(&base))).await?;
                unchanged(&string(&b["name"])?, &base)?;
                let sha = commit_sha(&b["commit"]["sha"])?;
                match self.get(&format!("{root}/branches/{}", enc(&name))).await {
                    Err(error) if error.code == "not_found" => {}
                    Err(error) => return Err(error),
                    Ok(_) => return Err(ForgeError::fixed(
                        "selection_changed",
                        "The new branch already exists; choose a different name and prepare again",
                    )),
                }
                observed["base_sha"] = json!(sha);
                observed["base_branch"] = json!(base);
                observed["new_branch"] = json!(name);
                observed["new_branch_absent"] = json!(true);
                validation.base_sha = Some(sha.clone());
                (
                    Method::POST,
                    format!("{root}/git/refs"),
                    json!({"ref":format!("refs/heads/{name}"),"sha":sha}),
                )
            }
            ForgeAction::ReviewCreate => {
                let head = branch(&req.input.head)?;
                let base = branch(&req.input.base)?;
                let h = self.get(&format!("{root}/branches/{}", enc(&head))).await?;
                let b = self.get(&format!("{root}/branches/{}", enc(&base))).await?;
                unchanged(&string(&h["name"])?, &head)?;
                unchanged(&string(&b["name"])?, &base)?;
                observed["head_sha"] = json!(string(&h["commit"]["sha"])?);
                observed["base_sha"] = json!(string(&b["commit"]["sha"])?);
                observed["head_branch"] = json!(head);
                observed["base_branch"] = json!(base);
                (
                    Method::POST,
                    format!("{root}/pulls"),
                    json!({"title":title(req)?,"body":body(req),"head":head,"base":base,"draft":req.input.draft.unwrap_or(false)}),
                )
            }
            ForgeAction::IssueCreate => {
                observed["label_ids"] = json!(
                    self.labels(req, &format!("{root}/labels?per_page=100"), false)
                        .await?
                );
                (
                    Method::POST,
                    format!("{root}/issues"),
                    json!({"title":title(req)?,"body":body(req),"labels":req.input.labels}),
                )
            }
            action => {
                let n = number(req)?;
                let endpoint = if action == ForgeAction::ReviewComment {
                    "pulls"
                } else {
                    "issues"
                };
                let row = self.get(&format!("{root}/{endpoint}/{n}")).await?;
                unchanged(&id(&row["number"])?, &n)?;
                unchanged(
                    &string(&row["url"])?,
                    &format!("{}{root}/{endpoint}/{n}", self.origin),
                )?;
                let oid = id(&row["id"])?;
                validation.object_id = Some(oid.clone());
                observed["object_id"] = json!(oid);
                observed["state"] = json!(string(&row["state"])?);
                observed["object_version"] = json!(hash(&row)?);
                if action == ForgeAction::ReviewComment {
                    unchanged(&id(&row["head"]["repo"]["id"])?, &pid)?;
                    unchanged(&id(&row["base"]["repo"]["id"])?, &pid)?;
                    observed["head_sha"] = json!(string(&row["head"]["sha"])?);
                    observed["base_sha"] = json!(string(&row["base"]["sha"])?);
                    observed["head_branch"] = json!(string(&row["head"]["ref"])?);
                    observed["base_branch"] = json!(string(&row["base"]["ref"])?);
                } else {
                    unchanged(
                        &string(&row["repository_url"])?,
                        &format!("{}{root}", self.origin),
                    )?;
                    if row.get("pull_request").is_some() {
                        return Err(ForgeError::invalid());
                    }
                }
                if action == ForgeAction::IssueSetState {
                    (
                        Method::PATCH,
                        format!("{root}/issues/{n}"),
                        json!({"state":state(req)?}),
                    )
                } else {
                    (
                        Method::POST,
                        format!("{root}/issues/{n}/comments"),
                        json!({"body":body(req)}),
                    )
                }
            }
        };
        Ok(Plan {
            observed,
            method,
            path,
            payload,
            validation,
        })
    }
    async fn labels(
        &self,
        req: &ForgeRequest,
        path: &str,
        jira: bool,
    ) -> Result<Vec<String>, ForgeError> {
        if req.input.labels.is_empty() {
            return Ok(Vec::new());
        }
        let value = self.get(path).await?;
        let list = if jira {
            rows(&value, "values")?
        } else {
            rows(&value, "")?
        };
        let mut selected_ids = Vec::new();
        for selected in &req.input.labels {
            let hits = list
                .iter()
                .filter(|v| {
                    if jira {
                        v.as_str() == Some(selected)
                    } else {
                        v["name"].as_str() == Some(selected)
                    }
                })
                .count();
            if hits != 1 {
                return Err(ForgeError::fixed(
                    "labels_unavailable",
                    "Every selected label must be an exact existing provider label",
                ));
            }
            let row = list
                .iter()
                .find(|v| {
                    if jira {
                        v.as_str() == Some(selected.as_str())
                    } else {
                        v["name"].as_str() == Some(selected.as_str())
                    }
                })
                .ok_or_else(ForgeError::upstream)?;
            selected_ids.push(if jira {
                selected.clone()
            } else {
                id(&row["id"])?
            });
        }
        Ok(selected_ids)
    }
    async fn gitlab(&self, req: &ForgeRequest) -> Result<Plan, ForgeError> {
        crate::operator_issues::validate_project(&crate::IssueRef {
            tracker: self.provider,
            project: req.target.project_ref.clone(),
            number: 1,
        })
        .map_err(|_| ForgeError::invalid())?;
        if req.input.labels.iter().any(|s| s.contains(',')) {
            return Err(ForgeError::invalid());
        }
        let repo = self
            .get(&format!("/projects/{}", enc(&req.target.project_ref)))
            .await?;
        unchanged(
            &string(&repo["path_with_namespace"])?,
            &req.target.project_ref,
        )?;
        let pid = id(&repo["id"])?;
        let root = format!("/projects/{pid}");
        let mut observed = json!({"provider_project_id":pid});
        let mut validation = self.validation(req, pid.clone(), None);
        let (method, path, payload) = match req.action {
            ForgeAction::BranchCreate => {
                let name = create_branch_name(&req.input.head)?;
                let base = create_branch_name(&req.input.base)?;
                let b = self
                    .get(&format!("{root}/repository/branches/{}", enc(&base)))
                    .await?;
                unchanged(&string(&b["name"])?, &base)?;
                let sha = commit_sha(&b["commit"]["id"])?;
                match self
                    .get(&format!("{root}/repository/branches/{}", enc(&name)))
                    .await
                {
                    Err(error) if error.code == "not_found" => {}
                    Err(error) => return Err(error),
                    Ok(_) => return Err(ForgeError::fixed(
                        "selection_changed",
                        "The new branch already exists; choose a different name and prepare again",
                    )),
                }
                observed["base_sha"] = json!(sha);
                observed["base_branch"] = json!(base);
                observed["new_branch"] = json!(name);
                observed["new_branch_absent"] = json!(true);
                validation.base_sha = Some(sha.clone());
                (
                    Method::POST,
                    format!("{root}/repository/branches"),
                    json!({"branch":name,"ref":sha}),
                )
            }
            ForgeAction::ReviewCreate => {
                let head = branch(&req.input.head)?;
                let base = branch(&req.input.base)?;
                let h = self
                    .get(&format!("{root}/repository/branches/{}", enc(&head)))
                    .await?;
                let b = self
                    .get(&format!("{root}/repository/branches/{}", enc(&base)))
                    .await?;
                unchanged(&string(&h["name"])?, &head)?;
                unchanged(&string(&b["name"])?, &base)?;
                observed["head_sha"] = json!(string(&h["commit"]["id"])?);
                observed["base_sha"] = json!(string(&b["commit"]["id"])?);
                observed["head_branch"] = json!(head);
                observed["base_branch"] = json!(base);
                if !req.input.draft.unwrap_or(false)
                    && (title(req)?.starts_with("Draft: ") || title(req)?.starts_with("WIP: "))
                {
                    return Err(ForgeError::fixed(
                        "invalid_intent",
                        "The reviewed title requests a draft but draft is not selected",
                    ));
                }
                if req.input.draft.unwrap_or(false) && !title(req)?.starts_with("Draft: ") {
                    return Err(ForgeError::fixed(
                        "invalid_intent",
                        "A GitLab draft requires an explicitly reviewed title beginning Draft: ",
                    ));
                }
                (
                    Method::POST,
                    format!("{root}/merge_requests"),
                    json!({"title":title(req)?,"description":body(req),"source_branch":head,"target_branch":base,"draft":req.input.draft.unwrap_or(false)}),
                )
            }
            ForgeAction::IssueCreate => {
                observed["label_ids"] = json!(
                    self.labels(req, &format!("{root}/labels?per_page=100&page=1"), false)
                        .await?
                );
                (
                    Method::POST,
                    format!("{root}/issues"),
                    json!({"title":title(req)?,"description":body(req),"labels":req.input.labels.join(",")}),
                )
            }
            action => {
                let n = number(req)?;
                let kind = if action == ForgeAction::ReviewComment {
                    "merge_requests"
                } else {
                    "issues"
                };
                let row = self.get(&format!("{root}/{kind}/{n}")).await?;
                unchanged(&id(&row["project_id"])?, &pid)?;
                unchanged(&id(&row["iid"])?, &n)?;
                let oid = id(&row["id"])?;
                validation.object_id = Some(oid.clone());
                observed["object_id"] = json!(oid);
                observed["state"] = json!(string(&row["state"])?);
                observed["object_version"] = json!(hash(&row)?);
                if action == ForgeAction::ReviewComment {
                    unchanged(&id(&row["source_project_id"])?, &pid)?;
                    unchanged(&id(&row["target_project_id"])?, &pid)?;
                    observed["head_sha"] = json!(string(&row["sha"])?);
                    observed["base_sha"] = json!(string(&row["diff_refs"]["base_sha"])?);
                    observed["head_branch"] = json!(string(&row["source_branch"])?);
                    observed["base_branch"] = json!(string(&row["target_branch"])?);
                }
                if action == ForgeAction::IssueSetState {
                    (
                        Method::PUT,
                        format!("{root}/issues/{n}"),
                        json!({"state_event":if state(req)?=="closed"{"close"}else{"reopen"}}),
                    )
                } else {
                    (
                        Method::POST,
                        format!("{root}/{kind}/{n}/notes"),
                        json!({"body":body(req)}),
                    )
                }
            }
        };
        Ok(Plan {
            observed,
            method,
            path,
            payload,
            validation,
        })
    }
    async fn jira(&self, req: &ForgeRequest) -> Result<Plan, ForgeError> {
        if req.action.is_repository() {
            return Err(ForgeError::unsupported());
        }
        // The connector's configured-site check precedes session creation in CLI.
        let project = req
            .target
            .project_ref
            .rsplit('/')
            .next()
            .filter(|s| clean(s, 128) && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'))
            .ok_or_else(ForgeError::invalid)?;
        let p = self
            .get(&format!("/rest/api/3/project/{}", enc(project)))
            .await?;
        unchanged(&string(&p["key"])?, project)?;
        let pid = id(&p["id"])?;
        let mut observed = json!({"provider_project_id":pid,"project_key":project});
        let mut validation = self.validation(req, pid.clone(), None);
        validation.project = project.into();
        let (method, path, payload) = if req.action == ForgeAction::IssueCreate {
            observed["labels"] = json!(
                self.labels(req, "/rest/api/3/label?maxResults=100&startAt=0", true)
                    .await?
            );
            let raw = self
                .get(&format!(
                    "/rest/api/3/issue/createmeta/{pid}/issuetypes?maxResults=100&startAt=0"
                ))
                .await?;
            let entries = raw
                .get("issueTypes")
                .or_else(|| raw.get("values"))
                .and_then(Value::as_array)
                .filter(|v| v.len() < 100)
                .ok_or_else(ForgeError::upstream)?;
            if raw["isLast"].as_bool() != Some(true)
                && raw["total"].as_u64() != Some(entries.len() as u64)
            {
                return Err(ForgeError::fixed(
                    "choices_incomplete",
                    "The provider choice catalog is incomplete",
                ));
            }
            let options = entries
                .iter()
                .map(|v| {
                    Ok(ForgeChoice {
                        id: id(&v["id"])?,
                        name: string(&v["name"])?,
                        kind: "issue_type".into(),
                    })
                })
                .collect::<Result<Vec<_>, ForgeError>>()?;
            let selected = choices(req, options)?;
            observed["choice_id"] = json!(selected.id);
            observed["choice_name"] = json!(selected.name);
            (
                Method::POST,
                "/rest/api/3/issue".into(),
                jira_create_payload(req, &pid, &selected.id)?,
            )
        } else {
            let key = format!("{project}-{}", number(req)?);
            let row=self.get(&format!("/rest/api/3/issue/{key}?fields=summary,description,status,labels,project,updated")).await?;
            unchanged(&string(&row["key"])?, &key)?;
            unchanged(&id(&row["fields"]["project"]["id"])?, &pid)?;
            let oid = id(&row["id"])?;
            validation.object_id = Some(oid.clone());
            observed["object_id"] = json!(oid);
            observed["key"] = json!(key);
            observed["state"] = json!(id(&row["fields"]["status"]["id"])?);
            observed["object_version"] = json!(hash(&row)?);
            if req.action == ForgeAction::IssueComment {
                (
                    Method::POST,
                    format!("/rest/api/3/issue/{oid}/comment"),
                    json!({"body":adf(&body(req))}),
                )
            } else {
                let transitions = self
                    .get(&format!("/rest/api/3/issue/{oid}/transitions"))
                    .await?;
                let entries = rows(&transitions, "transitions")?;
                let (selected, destination) = jira_transition(req, entries)?;
                observed["choice_id"] = json!(selected.id);
                observed["choice_name"] = json!(selected.name);
                observed["choice_destination"] = destination;
                (
                    Method::POST,
                    format!("/rest/api/3/issue/{oid}/transitions"),
                    json!({"transition":{"id":selected.id}}),
                )
            }
        };
        Ok(Plan {
            observed,
            method,
            path,
            payload,
            validation,
        })
    }
    async fn linear(&self, req: &ForgeRequest) -> Result<Plan, ForgeError> {
        if req.action.is_repository() {
            return Err(ForgeError::unsupported());
        }
        let data=self.graphql("query($id:String!){team(id:$id){id key states(first:100){nodes{id name type} pageInfo{hasNextPage}} labels(first:100){nodes{id name} pageInfo{hasNextPage}}}}",json!({"id":req.target.project_ref})).await?;
        let team = &data["team"];
        let pid = id(&team["id"])?;
        let key = string(&team["key"])?;
        if req.target.project_ref != pid && req.target.project_ref != key {
            return Err(ForgeError::changed());
        }
        let mut observed = json!({"provider_project_id":pid,"project_key":key});
        let mut validation = self.validation(req, pid.clone(), None);
        validation.project = key.clone();
        let (query, variables) = if req.action == ForgeAction::IssueCreate {
            if req.input.choice_id.is_some() {
                return Err(ForgeError::invalid());
            }
            let labels = rows(&team["labels"], "nodes")?;
            let mut ids = Vec::new();
            for name in &req.input.labels {
                let found: Vec<_> = labels
                    .iter()
                    .filter(|v| v["name"].as_str() == Some(name))
                    .collect();
                if found.len() != 1 {
                    return Err(ForgeError::fixed(
                        "labels_unavailable",
                        "Every selected label must be an exact existing provider label",
                    ));
                }
                ids.push(id(&found[0]["id"])?);
            }
            observed["label_ids"] = json!(ids);
            ("mutation($team:String!,$title:String!,$description:String!,$labels:[String!]){issueCreate(input:{teamId:$team,title:$title,description:$description,labelIds:$labels}){success issue{id identifier team{id key}}}}",json!({"team":pid,"title":title(req)?,"description":body(req),"labels":ids}))
        } else {
            let identifier = format!("{key}-{}", number(req)?);
            let data=self.graphql("query($id:String!){issue(id:$id){id identifier title description updatedAt team{id key} state{id type} labels(first:65){nodes{id name}}}}",json!({"id":identifier})).await?;
            let row = &data["issue"];
            unchanged(&string(&row["identifier"])?, &identifier)?;
            unchanged(&id(&row["team"]["id"])?, &pid)?;
            unchanged(&string(&row["team"]["key"])?, &key)?;
            let oid = id(&row["id"])?;
            validation.object_id = Some(oid.clone());
            observed["object_id"] = json!(oid);
            observed["identifier"] = json!(identifier);
            observed["state"] = json!(id(&row["state"]["id"])?);
            observed["object_version"] = json!(hash(row)?);
            if req.action == ForgeAction::IssueComment {
                ("mutation($issue:String!,$body:String!){commentCreate(input:{issueId:$issue,body:$body}){success comment{id issue{id team{id key}}}}}",json!({"issue":oid,"body":body(req)}))
            } else {
                if team["states"]["pageInfo"]["hasNextPage"].as_bool() != Some(false) {
                    return Err(ForgeError::fixed(
                        "choices_incomplete",
                        "The provider state catalog is incomplete",
                    ));
                }
                let desired = state(req)?;
                let options = rows(&team["states"], "nodes")?
                    .iter()
                    .filter(|v| {
                        v["type"].as_str().is_some_and(|kind| {
                            if desired == "closed" {
                                matches!(kind, "completed" | "canceled")
                            } else {
                                matches!(kind, "unstarted" | "started" | "backlog")
                            }
                        })
                    })
                    .map(|v| {
                        Ok(ForgeChoice {
                            id: id(&v["id"])?,
                            name: string(&v["name"])?,
                            kind: "state".into(),
                        })
                    })
                    .collect::<Result<Vec<_>, ForgeError>>()?;
                let selected = choices(req, options)?;
                validation.state_id = Some(selected.id.clone());
                observed["choice_id"] = json!(selected.id);
                observed["choice_name"] = json!(selected.name);
                ("mutation($issue:String!,$state:String!){issueUpdate(id:$issue,input:{stateId:$state}){success issue{id identifier team{id key} state{id}}}}",json!({"issue":oid,"state":selected.id}))
            }
        };
        Ok(Plan {
            observed,
            method: Method::POST,
            path: String::new(),
            payload: json!({"query":query,"variables":variables}),
            validation,
        })
    }
}
fn jira_transition(
    req: &ForgeRequest,
    entries: &[Value],
) -> Result<(ForgeChoice, Value), ForgeError> {
    let desired = state(req)?;
    let mut options = Vec::new();
    for row in entries {
        let category = string(&row["to"]["statusCategory"]["key"])?;
        if !matches!(category.as_str(), "new" | "indeterminate" | "done") {
            return Err(ForgeError::upstream());
        }
        // Capture complete selected destination identity, not only the transition label.
        id(&row["to"]["id"])?;
        string(&row["to"]["name"])?;
        if (category == "done") == (desired == "closed") {
            options.push(ForgeChoice {
                id: id(&row["id"])?,
                name: string(&row["name"])?,
                kind: "transition".into(),
            });
        }
    }
    let selected = choices(req, options)?;
    let row = entries
        .iter()
        .find(|r| id(&r["id"]).ok().as_ref() == Some(&selected.id))
        .ok_or_else(ForgeError::changed)?;
    let destination = json!({"id":id(&row["to"]["id"])?,"name":string(&row["to"]["name"])?,"category":string(&row["to"]["statusCategory"]["key"])?});
    Ok((selected, destination))
}
fn jira_create_payload(req: &ForgeRequest, pid: &str, type_id: &str) -> Result<Value, ForgeError> {
    let mut fields = json!({"project":{"id":pid},"issuetype":{"id":type_id},"summary":title(req)?,"labels":req.input.labels});
    if let Some(text) = &req.input.body {
        fields["description"] = adf(text);
    }
    Ok(json!({"fields":fields}))
}
fn adf(text: &str) -> Value {
    if text.is_empty() {
        return json!({"type":"doc","version":1,"content":[{"type":"paragraph","content":[]}]});
    }
    json!({"type":"doc","version":1,"content":[{"type":"paragraph","content":[{"type":"text","text":text}]}]})
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request() -> ForgeRequest {
        ForgeRequest {
            action: ForgeAction::IssueComment,
            target: ForgeTarget {
                provider: IssueTracker::Github,
                account: "work".into(),
                api_origin: "https://api.github.com".into(),
                project_ref: "org/repo".into(),
                number: Some("7".into()),
                ws_id: None,
            },
            expected: BTreeMap::from([("version".into(), "v".into())]),
            input: ForgeInput {
                body: Some("exact text".into()),
                ..Default::default()
            },
        }
    }
    #[test]
    fn exact_request_rejects_ignored_expectations_and_alias_numbers() {
        assert!(validate_request(&request()).is_ok());
        let mut r = request();
        r.target.number = Some("007".into());
        assert!(validate_request(&r).is_err());
        let mut r = request();
        r.expected.insert("head_sha".into(), "a".into());
        assert!(validate_request(&r).is_err());
        let mut r = request();
        r.target.ws_id = Some("guessed-workspace".into());
        assert!(validate_request(&r).is_err());
        let mut r = request();
        r.input.title = Some("ignored".into());
        assert!(validate_request(&r).is_err());
    }
    #[test]
    fn remote_ids_cannot_become_request_paths() {
        for value in [
            "12/path",
            "12?query",
            "../other",
            "https://other",
            "secret@host",
        ] {
            assert!(id(&json!(value)).is_err());
        }
        assert_eq!(id(&json!(42)).unwrap(), "42");
        assert_eq!(id(&json!("team-uuid")).unwrap(), "team-uuid");
    }
    #[test]
    fn choices_require_explicit_identity_even_when_one_remains() {
        let r = request();
        let option = || ForgeChoice {
            id: "transition-3".into(),
            name: "Close".into(),
            kind: "transition".into(),
        };
        assert_eq!(
            choices(&r, vec![option()]).unwrap_err().code,
            "choice_required"
        );
        let mut r = r;
        r.input.choice_id = Some("other".into());
        assert_eq!(
            choices(&r, vec![option()]).unwrap_err().code,
            "selection_changed"
        );
        r.input.choice_id = Some("transition-3".into());
        assert_eq!(choices(&r, vec![option()]).unwrap().id, "transition-3");
    }
    #[test]
    fn jira_transition_binds_destination_and_rejects_unknown_category() {
        let mut req = request();
        req.input.state = Some("closed".into());
        req.input.choice_id = Some("5".into());
        let original = json!({"id":"5","name":"Close","to":{"id":"9","name":"Done","statusCategory":{"key":"done"}}});
        let before = jira_transition(&req, &[original.clone()]).unwrap().1;
        let mut changed = original.clone();
        changed["to"]["id"] = json!("10");
        assert_ne!(before, jira_transition(&req, &[changed]).unwrap().1);
        let mut bad = original;
        bad["to"]["statusCategory"]["key"] = json!("unknown");
        req.input.state = Some("open".into());
        assert!(jira_transition(&req, &[bad]).is_err());
    }
    #[test]
    fn jira_create_absent_and_empty_description_never_emit_empty_text() {
        let mut req = request();
        req.input.title = Some("Title".into());
        req.input.body = None;
        assert!(jira_create_payload(&req, "1", "2").unwrap()["fields"]
            .get("description")
            .is_none());
        req.input.body = Some(String::new());
        let payload = jira_create_payload(&req, "1", "2").unwrap();
        assert_eq!(
            payload["fields"]["description"]["content"][0]["content"],
            json!([])
        );
        req.input.body = Some("Exact text".into());
        assert_eq!(
            jira_create_payload(&req, "1", "2").unwrap()["fields"]["description"]["content"][0]
                ["content"][0]["text"],
            "Exact text"
        );
    }
    #[test]
    fn gitlab_quick_action_lines_are_refused_without_rewriting_text() {
        let mut req = request();
        req.target.provider = IssueTracker::Gitlab;
        req.target.api_origin = "https://gitlab.example/api/v4".into();
        for text in [
            "/merge",
            "  /close",
            "\t/merge",
            "First line\n  /close",
            "ordinary\r/merge",
            "ordinary\u{2028}/merge",
            "ordinary\u{0085}/close",
            "\n/merge\nlast line",
        ] {
            req.input.body = Some(text.into());
            assert_eq!(
                validate_request(&req).unwrap_err().code,
                "unsupported_quick_action"
            );
            assert_eq!(req.input.body.as_deref(), Some(text));
        }
        for text in [
            "https://gitlab.example/org/repo",
            "See /merge documentation",
            "Text\nhttps://example.com/path",
        ] {
            req.input.body = Some(text.into());
            assert!(validate_request(&req).is_ok());
        }
        req.action = ForgeAction::IssueCreate;
        req.target.number = None;
        req.expected.clear();
        req.input.title = Some("Title".into());
        req.input.body = Some("Description\n /merge".into());
        assert_eq!(
            validate_request(&req).unwrap_err().code,
            "unsupported_quick_action"
        );
    }
    #[test]
    fn branch_creation_requires_repository_owner_and_safe_branch_names_only() {
        let mut req = request();
        req.action = ForgeAction::BranchCreate;
        req.target.number = None;
        req.target.ws_id = Some("workspace".into());
        req.expected.clear();
        req.input = ForgeInput {
            head: Some("feature/new".into()),
            base: Some("main".into()),
            ..Default::default()
        };
        assert!(validate_request(&req).is_ok());
        for name in [
            "",
            "@",
            "HEAD",
            "HEAD~1",
            "refs/heads/new",
            "a//b",
            "a/",
            ".hidden",
            "a.lock",
            "a/.b",
            "a@{x}",
            "a.",
            "a b",
            "a\nb",
            "../escape",
            "main",
        ] {
            let mut bad = req.clone();
            bad.input.head = Some(name.into());
            assert!(validate_request(&bad).is_err(), "{name:?}");
        }
        for provider in [IssueTracker::Jira, IssueTracker::Linear] {
            let mut bad = req.clone();
            bad.target.provider = provider;
            assert!(validate_request(&bad).is_err());
        }
        let mut bad = req.clone();
        bad.target.ws_id = None;
        assert!(validate_request(&bad).is_err());
        let mut bad = req.clone();
        bad.input.body = Some("unexpected".into());
        assert!(validate_request(&bad).is_err());
        let mut bad = req;
        bad.expected.insert("head_sha".into(), "a".repeat(40));
        assert!(validate_request(&bad).is_err());
    }
}
