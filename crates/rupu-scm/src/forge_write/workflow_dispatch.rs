//! GitHub Actions dispatch through the guarded operator transport only.
use super::*;

fn selector(input: &Option<String>) -> Result<String, ForgeError> {
    let value = input
        .as_ref()
        .filter(|v| clean(v, 255))
        .ok_or_else(ForgeError::invalid)?;
    let numeric = value
        .parse::<u64>()
        .ok()
        .filter(|v| *v > 0)
        .map(|v| v.to_string())
        .as_deref()
        == Some(value);
    let filename = value
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
        && (value.ends_with(".yml") || value.ends_with(".yaml"))
        && !value.starts_with('.')
        && !value.contains("..");
    if !numeric && !filename {
        return Err(ForgeError::invalid());
    }
    Ok(value.clone())
}
pub(super) fn validate(req: &ForgeRequest) -> Result<(), ForgeError> {
    let input = &req.input;
    selector(&input.workflow)?;
    create_branch_name(&input.ref_name)?;
    if req.target.provider != IssueTracker::Github
        || !matches!(input.ref_kind.as_deref(), Some("branch" | "tag"))
        || input.title.is_some()
        || input.body.is_some()
        || !input.labels.is_empty()
        || input.head.is_some()
        || input.base.is_some()
        || input.draft.is_some()
        || input.state.is_some()
        || input.choice_id.is_some()
        || input.workflow_inputs.len() > 25
        || input.workflow_inputs.iter().any(|(k, v)| {
            k.is_empty()
                || k.len() > 128
                || !k
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
                || v.len() > 8192
                || v.contains('\0')
        })
        || input
            .workflow_inputs
            .iter()
            .map(|(k, v)| k.len() + v.len())
            .sum::<usize>()
            > 65_536
    {
        return Err(ForgeError::invalid());
    }
    Ok(())
}
impl ForgeSession {
    pub(super) async fn github_workflow_dispatch(
        &self,
        req: &ForgeRequest,
        root: &str,
        project_id: String,
    ) -> Result<Plan, ForgeError> {
        let selected = selector(&req.input.workflow)?;
        let workflow = self
            .get(&format!("{root}/actions/workflows/{}", enc(&selected)))
            .await?;
        let workflow_id = id(&workflow["id"])?;
        if workflow_id
            .parse::<u64>()
            .ok()
            .filter(|v| *v > 0)
            .map(|v| v.to_string())
            .as_deref()
            != Some(workflow_id.as_str())
        {
            return Err(ForgeError::upstream());
        }
        unchanged(
            &string(&workflow["url"])?,
            &format!("{}{root}/actions/workflows/{workflow_id}", self.origin),
        )?;
        let workflow_path = string(&workflow["path"])?;
        let filename = workflow_path
            .strip_prefix(".github/workflows/")
            .ok_or_else(ForgeError::upstream)?;
        selector(&Some(filename.into())).map_err(|_| ForgeError::upstream())?;
        if !filename.ends_with(".yml") && !filename.ends_with(".yaml") {
            return Err(ForgeError::upstream());
        }
        if selected.bytes().all(|b| b.is_ascii_digit()) {
            unchanged(&workflow_id, &selected)?;
        } else {
            unchanged(filename, &selected)?;
        }
        let workflow_state = string(&workflow["state"])?;
        if workflow_state != "active" {
            return Err(ForgeError::fixed(
                "selection_changed",
                "The selected workflow is not active",
            ));
        }
        let ref_name = create_branch_name(&req.input.ref_name)?;
        let ref_kind = req
            .input
            .ref_kind
            .as_deref()
            .ok_or_else(ForgeError::invalid)?;
        let (namespace, opposite) = if ref_kind == "branch" {
            ("heads", "tags")
        } else {
            ("tags", "heads")
        };
        let ref_path = format!("{root}/git/ref/{namespace}/{}", enc(&ref_name));
        let reference = self.get(&ref_path).await?;
        unchanged(
            &string(&reference["ref"])?,
            &format!("refs/{namespace}/{ref_name}"),
        )?;
        // GitHub's ref response URL uses plural refs and unescaped path segments.
        let mut expected_url =
            url::Url::parse(&format!("{}{root}/git/refs/{namespace}/", self.origin))
                .map_err(|_| ForgeError::upstream())?;
        expected_url
            .path_segments_mut()
            .map_err(|_| ForgeError::upstream())?
            .pop_if_empty()
            .extend(ref_name.split('/'));
        if url::Url::parse(&string(&reference["url"])?).map_err(|_| ForgeError::upstream())?
            != expected_url
        {
            return Err(ForgeError::changed());
        }
        let ref_sha = commit_sha(&reference["object"]["sha"])?;
        let ref_object_type = string(&reference["object"]["type"])?;
        if (ref_kind == "branch" && ref_object_type != "commit")
            || (ref_kind == "tag" && !matches!(ref_object_type.as_str(), "commit" | "tag"))
        {
            return Err(ForgeError::upstream());
        }
        match self
            .get(&format!("{root}/git/ref/{opposite}/{}", enc(&ref_name)))
            .await
        {
            Err(error) if error.code == "not_found" => {}
            Err(error) => return Err(error),
            Ok(_) => {
                return Err(ForgeError::fixed(
                    "selection_changed",
                    "A branch and tag share this ref name; select an unambiguous ref",
                ))
            }
        }
        let proof = json!({"provider_project_id":project_id,"workflow_id":workflow_id,"workflow_path":workflow_path,
            "workflow_state":workflow_state,"ref_name":ref_name,"ref_kind":ref_kind,"ref_sha":ref_sha,"ref_object_type":ref_object_type});
        let mut observed = proof.clone();
        observed["object_id"] = json!(workflow_id);
        let mut validation = self.validation(req, project_id, Some(workflow_id.clone()));
        validation.workflow_dispatch = Some(proof);
        Ok(Plan {
            observed,
            method: Method::POST,
            path: format!("{root}/actions/workflows/{workflow_id}/dispatches"),
            payload: json!({"ref":ref_name,"inputs":req.input.workflow_inputs,"return_run_details":false}),
            validation,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::prelude::*;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    fn request(origin: String) -> ForgeRequest {
        ForgeRequest {
            action: ForgeAction::WorkflowDispatch,
            target: ForgeTarget {
                provider: IssueTracker::Github,
                account: "work".into(),
                api_origin: origin,
                project_ref: "org/repo".into(),
                number: None,
                ws_id: Some("workspace".into()),
            },
            expected: BTreeMap::new(),
            input: ForgeInput {
                workflow: Some("build.yml".into()),
                ref_name: Some("main".into()),
                ref_kind: Some("branch".into()),
                workflow_inputs: BTreeMap::from([("flag".into(), "false".into())]),
                ..Default::default()
            },
        }
    }
    fn session(server: &MockServer, epoch: Arc<AtomicUsize>) -> ForgeSession {
        crate::install_default_crypto_provider();
        let mut auth = reqwest::header::HeaderMap::new();
        auth.insert(
            reqwest::header::AUTHORIZATION,
            reqwest::header::HeaderValue::from_static("Bearer fixture"),
        );
        ForgeSession::new(
            IssueTracker::Github,
            server.base_url(),
            "1".into(),
            crate::forge_write::http("github", Arc::new(rupu_netflow::NullSink)).unwrap(),
            auth,
            Arc::new(move || epoch.load(Ordering::SeqCst).to_string()),
        )
        .unwrap()
    }
    fn repo(server: &MockServer) -> httpmock::Mock<'_> {
        server.mock(|w, t| {
            w.method(GET).path("/repos/org/repo");
            t.status(200).json_body(
                json!({"id":1,"full_name":"org/repo","url":server.url("/repos/org/repo")}),
            );
        })
    }
    fn workflow<'a>(
        server: &'a MockServer,
        id: u64,
        state: &str,
        path: &str,
    ) -> httpmock::Mock<'a> {
        server.mock(|w,t| {w.method(GET).path("/repos/org/repo/actions/workflows/build.yml").header("x-github-api-version","2022-11-28");t.status(200).json_body(json!({"id":id,"state":state,"path":path,"url":server.url(format!("/repos/org/repo/actions/workflows/{id}"))}));})
    }
    fn reference<'a>(
        server: &'a MockServer,
        namespace: &str,
        sha: &str,
        kind: &str,
    ) -> httpmock::Mock<'a> {
        server.mock(|w,t| {w.method(GET).path(format!("/repos/org/repo/git/ref/{namespace}/main"));t.status(200).json_body(json!({"ref":format!("refs/{namespace}/main"),"url":server.url(format!("/repos/org/repo/git/refs/{namespace}/main")),"object":{"sha":sha,"type":kind}}));})
    }
    fn absent<'a>(server: &'a MockServer, namespace: &str, status: u16) -> httpmock::Mock<'a> {
        server.mock(|w, t| {
            w.method(GET)
                .path(format!("/repos/org/repo/git/ref/{namespace}/main"));
            t.status(status).json_body(json!({"message":"fixture"}));
        })
    }
    #[test]
    fn dispatch_requires_explicit_safe_scope_workflow_ref_and_bounded_string_inputs() {
        let req = request("https://api.github.com".into());
        assert!(validate_request(&req).is_ok());
        for value in [
            "0",
            "01",
            "18446744073709551616",
            "../build.yml",
            ".hidden.yml",
            "a..yml",
            "build.txt",
            "a/b.yml",
        ] {
            let mut bad = req.clone();
            bad.input.workflow = Some(value.into());
            assert!(validate_request(&bad).is_err(), "{value}");
        }
        for value in ["HEAD", "refs/heads/main", "main~1", "a..b", "a b", "a@{x}"] {
            let mut bad = req.clone();
            bad.input.ref_name = Some(value.into());
            assert!(validate_request(&bad).is_err());
        }
        for provider in [
            IssueTracker::Gitlab,
            IssueTracker::Jira,
            IssueTracker::Linear,
        ] {
            let mut bad = req.clone();
            bad.target.provider = provider;
            assert!(validate_request(&bad).is_err());
        }
        let mut bad = req.clone();
        bad.target.ws_id = None;
        assert!(validate_request(&bad).is_err());
        let mut bad = req.clone();
        bad.input.ref_kind = None;
        assert!(validate_request(&bad).is_err());
        let mut bad = req.clone();
        bad.input
            .workflow_inputs
            .insert("bad key".into(), "x".into());
        assert!(validate_request(&bad).is_err());
        let mut bad = req.clone();
        bad.input
            .workflow_inputs
            .insert("key".into(), "x".repeat(8193));
        assert!(validate_request(&bad).is_err());
        let mut bad = req.clone();
        bad.input.workflow_inputs = (0..26).map(|n| (format!("k{n}"), "x".into())).collect();
        assert!(validate_request(&bad).is_err());
        let mut bad = req.clone();
        bad.input.workflow_inputs = (0..9)
            .map(|n| (format!("k{n}"), "x".repeat(8192)))
            .collect();
        assert!(validate_request(&bad).is_err());
        let mut bad = req.clone();
        bad.input.body = Some("ignored".into());
        assert!(validate_request(&bad).is_err());
        let mut bad = req.clone();
        bad.action = ForgeAction::BranchCreate;
        bad.input.head = Some("new".into());
        bad.input.base = Some("main".into());
        assert!(validate_request(&bad).is_err());
    }
    #[test]
    fn new_optional_input_fields_do_not_change_old_serialized_intent_shape() {
        let raw = json!({"title":null,"body":null,"labels":[],"head":null,"base":null,"draft":null,"state":null,"choice_id":null});
        let old: ForgeInput = serde_json::from_value(raw.clone()).unwrap();
        assert_eq!(serde_json::to_value(old).unwrap(), raw);
        let mut request = serde_json::to_value(request("https://api.github.com".into())).unwrap();
        request["input"]["workflow_inputs"] = json!({"flag":false});
        assert!(serde_json::from_value::<ForgeRequest>(request).is_err());
    }
    #[tokio::test]
    async fn dispatch_prepares_rechecks_and_acknowledges_without_inventing_a_run() {
        let server = MockServer::start_async().await;
        let _repo = repo(&server);
        let flow = workflow(&server, 42, "active", ".github/workflows/build.yml");
        let selected = reference(&server, "heads", &"a".repeat(40), "commit");
        let other = absent(&server, "tags", 404);
        let post = server.mock(|w, t| {
            w.method(POST)
                .path("/repos/org/repo/actions/workflows/42/dispatches")
                .header("x-github-api-version", "2022-11-28")
                .json_body(
                    json!({"ref":"main","inputs":{"flag":"false"},"return_run_details":false}),
                );
            t.status(204);
        });
        let epoch = Arc::new(AtomicUsize::new(1));
        let req = request(server.base_url());
        let prepared = session(&server, epoch.clone()).prepare(&req).await.unwrap();
        assert_eq!(prepared.request, req);
        assert_eq!(prepared.observed["workflow_id"], "42");
        assert_eq!(prepared.authority["provider_object_id"], "42");
        let result = session(&server, epoch)
            .preflight(&prepared)
            .await
            .unwrap()
            .dispatch()
            .await;
        assert_eq!(result.outcome, DispatchOutcome::Accepted);
        assert_eq!(result.result["verification"], "not_performed");
        assert_eq!(result.result["workflow_id"], "42");
        assert_eq!(result.result["ref_sha"], "a".repeat(40));
        assert!(
            result.result.get("run_id").is_none() && result.result.get("resource_id").is_none()
        );
        flow.assert_hits(2);
        selected.assert_hits(2);
        other.assert_hits(2);
        post.assert_hits(1);
    }
    #[tokio::test]
    async fn dispatch_rechecks_ref_workflow_and_namespace_before_sending() {
        for change in 0..3 {
            let server = MockServer::start_async().await;
            let _repo = repo(&server);
            let mut flow = workflow(&server, 42, "active", ".github/workflows/build.yml");
            let mut selected = reference(&server, "heads", &"a".repeat(40), "commit");
            let mut other = absent(&server, "tags", 404);
            let epoch = Arc::new(AtomicUsize::new(1));
            let prepared = session(&server, epoch.clone())
                .prepare(&request(server.base_url()))
                .await
                .unwrap();
            let _replacement = match change {
                0 => {
                    flow.delete();
                    workflow(&server, 43, "active", ".github/workflows/build.yml")
                }
                1 => {
                    selected.delete();
                    reference(&server, "heads", &"b".repeat(40), "commit")
                }
                _ => {
                    other.delete();
                    absent(&server, "tags", 200)
                }
            };
            let post = server.mock(|w, t| {
                w.method(POST);
                t.status(204);
            });
            assert!(session(&server, epoch).preflight(&prepared).await.is_err());
            post.assert_hits(0);
        }
    }
    #[tokio::test]
    async fn dispatch_refuses_wrong_path_inactive_workflow_and_failed_opposite_lookup() {
        for case in 0..3 {
            let server = MockServer::start_async().await;
            let _repo = repo(&server);
            let _flow = workflow(
                &server,
                42,
                if case == 0 {
                    "disabled_manually"
                } else {
                    "active"
                },
                if case == 1 {
                    ".github/workflows/other.yml"
                } else {
                    ".github/workflows/build.yml"
                },
            );
            let _selected = reference(&server, "heads", &"a".repeat(40), "commit");
            let _other = absent(&server, "tags", if case == 2 { 403 } else { 404 });
            assert!(session(&server, Arc::new(AtomicUsize::new(1)))
                .prepare(&request(server.base_url()))
                .await
                .is_err());
        }
    }
    #[tokio::test]
    async fn annotated_tag_retains_tag_object_sha_without_peeling_or_commit_claim() {
        let server = MockServer::start_async().await;
        let _repo = repo(&server);
        let _flow = workflow(&server, 42, "active", ".github/workflows/build.yml");
        let selected = reference(&server, "tags", &"b".repeat(40), "tag");
        let _other = absent(&server, "heads", 404);
        let mut req = request(server.base_url());
        req.input.ref_kind = Some("tag".into());
        let prepared = session(&server, Arc::new(AtomicUsize::new(1)))
            .prepare(&req)
            .await
            .unwrap();
        assert_eq!(prepared.observed["ref_object_type"], "tag");
        assert_eq!(prepared.observed["ref_sha"], "b".repeat(40));
        selected.assert_hits(1);
    }
    #[tokio::test]
    async fn dispatch_never_retries_and_only_empty_204_is_accepted() {
        for status in [200u16, 204, 401, 429, 500] {
            let server = MockServer::start_async().await;
            let _repo = repo(&server);
            let _flow = workflow(&server, 42, "active", ".github/workflows/build.yml");
            let _selected = reference(&server, "heads", &"a".repeat(40), "commit");
            let _other = absent(&server, "tags", 404);
            let post = server.mock(|w, t| {
                w.method(POST)
                    .path("/repos/org/repo/actions/workflows/42/dispatches");
                if status == 204 {
                    t.status(status);
                } else {
                    t.status(status).json_body(json!({"message":"fixture"}));
                }
            });
            let epoch = Arc::new(AtomicUsize::new(1));
            let prepared = session(&server, epoch.clone())
                .prepare(&request(server.base_url()))
                .await
                .unwrap();
            let result = session(&server, epoch)
                .preflight(&prepared)
                .await
                .unwrap()
                .dispatch()
                .await;
            assert_eq!(
                result.outcome,
                match status {
                    204 => DispatchOutcome::Accepted,
                    401 | 429 => DispatchOutcome::Rejected,
                    _ => DispatchOutcome::Unknown,
                }
            );
            post.assert_hits(1);
        }
    }
    #[tokio::test]
    async fn credential_generation_change_prevents_dispatch() {
        let server = MockServer::start_async().await;
        let _repo = repo(&server);
        let _flow = workflow(&server, 42, "active", ".github/workflows/build.yml");
        let _selected = reference(&server, "heads", &"a".repeat(40), "commit");
        let _other = absent(&server, "tags", 404);
        let post = server.mock(|w, t| {
            w.method(POST);
            t.status(204);
        });
        let epoch = Arc::new(AtomicUsize::new(1));
        let prepared = session(&server, epoch.clone())
            .prepare(&request(server.base_url()))
            .await
            .unwrap();
        let mutation = session(&server, epoch.clone())
            .preflight(&prepared)
            .await
            .unwrap();
        epoch.store(2, Ordering::SeqCst);
        assert_eq!(mutation.dispatch().await.outcome, DispatchOutcome::Unknown);
        post.assert_hits(0);
    }
    #[tokio::test]
    async fn numeric_workflow_selector_must_match_returned_immutable_id() {
        for (returned, path, accepted) in [
            (42, ".github/workflows/build.yml", true),
            (43, ".github/workflows/build.yml", false),
            (42, ".github/workflows/42", false),
        ] {
            let server = MockServer::start_async().await;
            let _repo = repo(&server);
            let _flow=server.mock(|w,t|{w.method(GET).path("/repos/org/repo/actions/workflows/42");t.status(200).json_body(json!({"id":returned,"state":"active","path":path,"url":server.url(format!("/repos/org/repo/actions/workflows/{returned}"))}));});
            let _selected = reference(&server, "heads", &"a".repeat(40), "commit");
            let _other = absent(&server, "tags", 404);
            let mut req = request(server.base_url());
            req.input.workflow = Some("42".into());
            let result = session(&server, Arc::new(AtomicUsize::new(1)))
                .prepare(&req)
                .await;
            assert_eq!(result.is_ok(), accepted);
        }
    }
    #[tokio::test]
    async fn dispatch_contradictory_rejection_retains_unknown_and_never_replays() {
        for key in [
            "workflow_run_id",
            "run_url",
            "html_url",
            "run_id",
            "check_suite_id",
            "workflow_runs",
        ] {
            let server = MockServer::start_async().await;
            let _repo = repo(&server);
            let _flow = workflow(&server, 42, "active", ".github/workflows/build.yml");
            let _selected = reference(&server, "heads", &"a".repeat(40), "commit");
            let _other = absent(&server, "tags", 404);
            let mut contradictory = json!({"message":"fixture error"});
            contradictory[key] = json!(123);
            let post = server.mock(|w, t| {
                w.method(POST)
                    .path("/repos/org/repo/actions/workflows/42/dispatches");
                t.status(422).json_body(contradictory);
            });
            let epoch = Arc::new(AtomicUsize::new(1));
            let prepared = session(&server, epoch.clone())
                .prepare(&request(server.base_url()))
                .await
                .unwrap();
            let result = session(&server, epoch)
                .preflight(&prepared)
                .await
                .unwrap()
                .dispatch()
                .await;
            assert_eq!(result.outcome, DispatchOutcome::Unknown);
            post.assert_hits(1);
        }
    }
}
