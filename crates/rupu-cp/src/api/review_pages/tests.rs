use super::*;
fn owner() -> Owner {
    Owner {
        ws: "ws_fixture".into(),
        cwd: "/fixture/project".into(),
        origin: "https://github.com/fixture/repo".into(),
        account: "fixture".into(),
        branch: "topic".into(),
        head: "head".into(),
        base: Some("base".into()),
        number: 9,
    }
}
fn identity() -> Identity {
    Identity {
        account: "fixture".into(),
        connector_epoch: "random-fixture-generation".into(),
        api_profile: "fixture-profile".into(),
        provider_version: None,
        repository_id: "repo_fixture".into(),
        number: 9,
        branch: "topic".into(),
        head: "head".into(),
        base: "base".into(),
        start: None,
        pipeline: None,
        file_count: Some(1),
    }
}
fn observation() -> Observation {
    Observation {
        owner: owner(),
        identity: identity(),
        created: Instant::now(),
        first: "2026-01-01T00:00:00Z".into(),
        state: AsyncMutex::new(Vec::new()),
    }
}
fn item(id: &str) -> Item {
    Item {
        id: id.into(),
        value: json!({"id":id,"body":"fixture"}),
        children: vec![],
    }
}
fn page(items: Vec<Item>) -> Page {
    Page {
        identity: identity(),
        scope: "fixture".into(),
        items,
        next: None,
        provider_exhausted: true,
        omissions: vec![],
        input_bytes: 10,
        page_version: None,
    }
}
fn query() -> String {
    url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs([
            ("expected_origin", "https://github.com/fixture/repo"),
            ("expected_account", "fixture"),
            ("expected_branch", "topic"),
            ("expected_head", "head"),
            ("section", "checks"),
        ])
        .finish()
}
#[test]
fn unknown_duplicate_nested_or_oversize_queries_refuse() {
    assert!(Query::parse(Some(query())).is_ok());
    for suffix in [
        "&section=threads",
        "&account=other",
        "&parent=node",
        "&host=local&host=remote",
        "&cursor=",
    ] {
        assert!(Query::parse(Some(format!("{}{suffix}", query()))).is_err());
    }
    assert!(Query::parse(Some(query().replace("section=checks", "section=comments"))).is_err());
    assert!(Query::parse(Some("x".repeat(16385))).is_err());
}
#[test]
fn every_expected_owner_component_is_bound() {
    let o = owner();
    let i = identity();
    assert!(valid_identity(&o, &i).is_ok());
    for field in ["account", "branch", "head", "base", "number"] {
        let mut altered = i.clone();
        match field {
            "account" => altered.account = "other".into(),
            "branch" => altered.branch = "other".into(),
            "head" => altered.head = "other".into(),
            "base" => altered.base = "other".into(),
            _ => altered.number = 10,
        };
        assert_eq!(
            valid_identity(&o, &altered).unwrap_err().0,
            StatusCode::CONFLICT
        );
    }
}
#[test]
fn captured_notes_continue_without_losing_extra_rows() {
    let o = observation();
    let mut state = Pages::default();
    let result = publish(
        &mut state,
        &o,
        "observation",
        "first",
        "comments",
        page((0..41).map(|n| item(&format!("note{n}"))).collect()),
    )
    .unwrap();
    assert_eq!(result["items"].as_array().unwrap().len(), 20);
    let cursor = result["next_cursor"].as_str().unwrap();
    let frame = state.frames[cursor.split_once('.').unwrap().1].clone();
    let next = captured(&identity(), frame.target.unwrap()).unwrap();
    assert_eq!(next.items[0].id, "note20");
    assert_eq!(next.items.len(), 21);
    let result = publish(&mut state, &o, "observation", "second", "comments", next).unwrap();
    assert_eq!(result["items"][19]["id"], "note39");
    let cursor = result["next_cursor"].as_str().unwrap();
    let frame = &state.frames[cursor.split_once('.').unwrap().1];
    let final_page = captured(&identity(), frame.target.clone().unwrap()).unwrap();
    assert_eq!(final_page.items[0].id, "note40");
}
#[test]
fn duplicate_page_or_output_overrun_does_not_advance_frontier() {
    let o = observation();
    let mut state = Pages::default();
    publish(
        &mut state,
        &o,
        "obs",
        "one",
        "checks",
        page(vec![item("a")]),
    )
    .unwrap();
    let before = serde_json::to_vec(&state).unwrap();
    assert!(publish(
        &mut state,
        &o,
        "obs",
        "two",
        "checks",
        page(vec![item("a")])
    )
    .is_err());
    assert_eq!(serde_json::to_vec(&state).unwrap(), before);
    let mut huge = item("b");
    huge.value["body"] = json!("x".repeat(MAX_OUTPUT));
    assert!(publish(&mut state, &o, "obs", "three", "checks", page(vec![huge])).is_err());
    assert_eq!(serde_json::to_vec(&state).unwrap(), before);
}
#[test]
fn nested_capabilities_keep_exact_captured_target_and_hide_provider_cursor() {
    let mut parent = item("thread");
    parent.children.push(Child {
        field: "comments_cursor".into(),
        section: "comments".into(),
        target: Target::Provider(Request {
            section: "comments".into(),
            parent: Some("thread".into()),
            after: Some("vendor-private-cursor".into()),
        }),
    });
    let o = observation();
    let mut state = Pages::default();
    let result = publish(
        &mut state,
        &o,
        "obs",
        "first",
        "threads",
        page(vec![parent]),
    )
    .unwrap();
    assert!(!result.to_string().contains("vendor-private-cursor"));
    let cursor = result["items"][0]["comments_cursor"].as_str().unwrap();
    let target = state.frames[cursor.split_once('.').unwrap().1]
        .target
        .as_ref()
        .unwrap();
    assert!(matches!(target,Target::Provider(Request{parent:Some(p),..})if p=="thread"));
}
#[test]
fn long_unicode_text_is_exactly_reconstructed_and_digest_stable() {
    let text = "x🦀é\n".repeat(50000);
    let mut offset = 0;
    let mut combined = String::new();
    let mut digest = None;
    loop {
        let p = captured(
            &identity(),
            Target::Text {
                id: "comment-fixture".into(),
                text: text.clone(),
                offset,
            },
        )
        .unwrap();
        let row = &p.items[0];
        combined.push_str(row.value["text"].as_str().unwrap());
        let hash = row.value["sha256"].as_str().unwrap();
        if let Some(d) = &digest {
            assert_eq!(d, hash);
        } else {
            digest = Some(hash.to_string());
        }
        offset = row.value["byte_end"].as_u64().unwrap() as usize;
        if row.children.is_empty() {
            break;
        }
    }
    assert_eq!(combined, text);
    assert!(captured(
        &identity(),
        Target::Text {
            id: "x".into(),
            text: "é".into(),
            offset: 1
        }
    )
    .is_err());
}
#[test]
fn old_published_pages_expire_instead_of_silent_refetch() {
    let o = observation();
    let mut state = Pages::default();
    for n in 0..17 {
        publish(
            &mut state,
            &o,
            "obs",
            &format!("p{n}"),
            "checks",
            page(vec![item(&format!("item{n}"))]),
        )
        .unwrap();
    }
    assert!(!state.frames.contains_key("p0"));
    assert_eq!(state.order.len(), 16);
    assert!(state.frames["p16"].target.is_none());
}
#[test]
fn partial_provider_omissions_and_finite_target_budget_stay_explicit() {
    let o = observation();
    let mut state = Pages::default();
    let mut p = page(vec![]);
    p.provider_exhausted = false;
    p.omissions.push("pagination_unreported".into());
    let value = publish(&mut state, &o, "obs", "p", "checks", p).unwrap();
    assert_eq!(value["partial"], true);
    assert_eq!(value["provider_exhausted"], false);
    assert!(value["next_cursor"].is_null());
    state.issued = MAX_FRAMES;
    assert_eq!(
        add_frame(
            &mut state,
            "obs",
            "checks".into(),
            Target::Captured {
                scope: "fixture".into(),
                items: vec![]
            }
        )
        .unwrap_err()
        .0,
        StatusCode::GONE
    );
}

#[test]
fn section_provider_version_change_requires_refresh_without_replacing_page() {
    let o = observation();
    let mut state = Pages::default();
    let mut first = page(vec![item("a")]);
    first.page_version = Some("vendor-v1".into());
    publish(&mut state, &o, "obs", "first", "diff_files", first).unwrap();
    let mut second = page(vec![item("b")]);
    second.page_version = Some("vendor-v2".into());
    assert_eq!(
        publish(&mut state, &o, "obs", "second", "diff_files", second)
            .unwrap_err()
            .0,
        StatusCode::CONFLICT
    );
    assert!(!state.frames.contains_key("second"));
    assert_eq!(state.versions["fixture"], Some("vendor-v1".into()));
}

struct ProviderSpy {
    reads: std::sync::atomic::AtomicUsize,
    epoch: std::sync::atomic::AtomicUsize,
    pause_next: std::sync::atomic::AtomicBool,
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
}
impl ProviderSpy {
    fn identity_for(&self, selected: &ReviewSelection) -> Identity {
        let mut id = identity();
        id.branch = selected.branch.clone();
        id.head = selected.head.clone();
        id.connector_epoch = self
            .epoch
            .load(std::sync::atomic::Ordering::SeqCst)
            .to_string();
        id
    }
}
#[async_trait::async_trait]
impl RepoLister for ProviderSpy {
    async fn list(&self) -> Result<Vec<crate::repos::RepoEntry>, RepoListError> {
        panic!("review continuation must not enumerate repositories")
    }
    fn supports_review_pages(&self) -> bool {
        true
    }
    async fn review_page_identity(
        &self,
        _: &rupu_scm::RepoRef,
        _: &FsPath,
        _: &str,
        _: u32,
        selected: &ReviewSelection,
    ) -> Result<Identity, RepoListError> {
        Ok(self.identity_for(selected))
    }
    async fn review_page(
        &self,
        _: &rupu_scm::RepoRef,
        _: &FsPath,
        origin: &str,
        number: u32,
        selected: &ReviewSelection,
        expected: Option<&Identity>,
        request: &Request,
    ) -> Result<Page, RepoListError> {
        assert_eq!(origin, "https://github.com/fixture/repo");
        assert_eq!(number, 9);
        let id = self.identity_for(selected);
        if expected.is_some_and(|e| e != &id) {
            return Err(RepoListError::SelectionChanged);
        }
        self.reads.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if self
            .pause_next
            .swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            self.entered.notify_one();
            self.release.notified().await;
        }
        tokio::task::yield_now().await;
        let mut p = page(vec![item(if request.after.is_none() {
            "first"
        } else {
            "second"
        })]);
        p.identity = id;
        if request.after.is_none() {
            p.next = Some(Request {
                section: "checks".into(),
                after: Some("vendor-second".into()),
                parent: None,
            });
            p.provider_exhausted = false;
        }
        Ok(p)
    }
}
#[tokio::test]
async fn http_scope_cached_replay_coalescing_epoch_and_eviction_are_bound() {
    let (_home, _project, state, ws_id, raw, spy) = fixture();
    let q = || Query::parse(Some(raw.clone())).unwrap();
    let first = read(state.clone(), ws_id.clone(), 9, q()).await.unwrap();
    assert_eq!(first["ws_id"], ws_id);
    let continuation = format!("{raw}&cursor={}", first["next_cursor"].as_str().unwrap());
    let q2 = || Query::parse(Some(continuation.clone())).unwrap();
    let (a, b) = tokio::join!(
        read(state.clone(), ws_id.clone(), 9, q2()),
        read(state.clone(), ws_id.clone(), 9, q2())
    );
    let a = a.unwrap();
    assert_eq!(a, b.unwrap());
    assert_eq!(a["items"][0]["id"], "second");
    assert_eq!(spy.reads.load(std::sync::atomic::Ordering::SeqCst), 2);
    assert_eq!(
        read(state.clone(), ws_id.clone(), 10, q2())
            .await
            .unwrap_err()
            .0,
        StatusCode::CONFLICT
    );
    let mut wrong_section = q2();
    wrong_section.section = "threads".into();
    assert_eq!(
        read(state.clone(), ws_id.clone(), 9, wrong_section)
            .await
            .unwrap_err()
            .0,
        StatusCode::CONFLICT
    );
    spy.epoch.store(2, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(
        read(state.clone(), ws_id.clone(), 9, q2())
            .await
            .unwrap_err()
            .0,
        StatusCode::CONFLICT
    );
    assert_eq!(spy.reads.load(std::sync::atomic::Ordering::SeqCst), 2);
    state.review_pages.entries.lock().unwrap().clear();
    assert_eq!(
        read(state.clone(), ws_id.clone(), 9, q2())
            .await
            .unwrap_err()
            .0,
        StatusCode::GONE
    );
    let remote = RawQuery(Some(format!("{raw}&host=remote")));
    assert!(get(State(state), Path((ws_id, 9)), remote).await.is_err());
}

fn fixture() -> (
    tempfile::TempDir,
    tempfile::TempDir,
    AppState,
    String,
    String,
    Arc<ProviderSpy>,
) {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let repo = git2::Repository::init(project.path()).unwrap();
    let tree_id = repo.treebuilder(None).unwrap().write().unwrap();
    let tree = repo.find_tree(tree_id).unwrap();
    let sig = git2::Signature::now("Fixture", "fixture@example.invalid").unwrap();
    repo.commit(Some("HEAD"), &sig, &sig, "fixture", &tree, &[])
        .unwrap();
    repo.remote("origin", "https://github.com/fixture/repo.git")
        .unwrap();
    let branch = repo.head().unwrap().shorthand().unwrap().to_string();
    let ws = rupu_workspace::register(
        &rupu_workspace::WorkspaceStore {
            root: home.path().join("workspaces"),
        },
        project.path(),
    )
    .unwrap();
    let spy = Arc::new(ProviderSpy {
        reads: std::sync::atomic::AtomicUsize::new(0),
        epoch: std::sync::atomic::AtomicUsize::new(1),
        pause_next: std::sync::atomic::AtomicBool::new(false),
        entered: tokio::sync::Notify::new(),
        release: tokio::sync::Notify::new(),
    });
    let state = AppState::new(home.path().into(), rupu_config::PricingConfig::default())
        .with_repos(Some(spy.clone()));
    let raw = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs([
            ("expected_origin", "https://github.com/fixture/repo"),
            ("expected_account", "fixture"),
            ("expected_branch", branch.as_str()),
            ("expected_head", "head"),
            ("section", "checks"),
        ])
        .finish();
    drop(tree);
    drop(repo);
    (home, project, state, ws.id, raw, spy)
}

#[tokio::test]
async fn cancelled_unpublished_page_keeps_exact_cursor_and_encoded_frontier() {
    let (_home, _project, state, ws, raw, spy) = fixture();
    let first = read(
        state.clone(),
        ws.clone(),
        9,
        Query::parse(Some(raw.clone())).unwrap(),
    )
    .await
    .unwrap();
    let cursor = first["next_cursor"].as_str().unwrap().to_owned();
    let observation =
        state.review_pages.entries.lock().unwrap()[cursor.split_once('.').unwrap().0].clone();
    let before = observation.state.lock().await.clone();
    spy.pause_next
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let continued = format!("{raw}&cursor={cursor}");
    let task = tokio::spawn(read(
        state.clone(),
        ws.clone(),
        9,
        Query::parse(Some(continued.clone())).unwrap(),
    ));
    tokio::time::timeout(Duration::from_secs(2), spy.entered.notified())
        .await
        .unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert_eq!(*observation.state.lock().await, before);
    let second = read(state, ws, 9, Query::parse(Some(continued)).unwrap())
        .await
        .unwrap();
    assert_eq!(second["items"][0]["id"], "second");
    assert_eq!(spy.reads.load(std::sync::atomic::Ordering::SeqCst), 3);
}
