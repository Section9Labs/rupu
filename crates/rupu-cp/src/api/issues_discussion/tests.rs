use super::*;
use crate::issue_reader::{IssueReadAccount, IssueReader};
use rupu_scm::{
    issue_discussion::DiscussionComment, operator_issues::IssueCommentPage, Issue, IssueTracker,
};
use std::sync::atomic::{AtomicUsize, Ordering};
fn selected() -> IssueReadTarget {
    IssueReadTarget {
        tracker: IssueTracker::Github,
        project: "org/repo".into(),
        account: "work".into(),
        number: 7,
        origin: "https://api.github.com".into(),
    }
}
fn query(cursor: Option<String>) -> DiscussionQuery {
    let t = selected();
    DiscussionQuery {
        host: Some("local".into()),
        project: t.project,
        account: t.account,
        expected_origin: t.origin,
        cursor,
    }
}
fn proof() -> DiscussionIdentity {
    DiscussionIdentity {
        connector_epoch: "epoch-0".into(),
        api_profile: "github:api.github.com".into(),
        project_id: "repo-1".into(),
        issue_id: "issue-7".into(),
        identifier: "7".into(),
    }
}
fn comment(id: &str, body: &str) -> DiscussionComment {
    DiscussionComment {
        id: id.into(),
        kind: "comment".into(),
        author: Some("Author".into()),
        created_at: "2026-11-09T00:00:00Z".into(),
        updated_at: None,
        parent_comment_id: None,
        body: body.into(),
        format: "markdown".into(),
        source_format: "markdown".into(),
        projection_complete: true,
        unsupported_nodes: 0,
        unsupported_marks: 0,
        metadata_truncated: false,
    }
}
fn provider(items: Vec<DiscussionComment>, next: Option<&str>) -> DiscussionPage {
    DiscussionPage {
        identity: proof(),
        items,
        next: next.map(str::to_string),
        order: "oldest_first".into(),
        provider_exhausted: next.is_none(),
        input_bytes: 100,
    }
}
fn observation() -> Observation {
    Observation {
        owner: "owner".into(),
        target: selected(),
        epoch: "epoch-0".into(),
        created: Instant::now(),
        observed_at: "2026-11-09T00:00:00Z".into(),
        expires_at: "2026-11-09T00:10:00Z".into(),
        state: AsyncMutex::new(vec![]),
    }
}
fn code<T>(r: Result<T>) -> &'static str {
    r.err().expect("expected refusal").1
}
#[test]
fn long_utf8_text_reassembles_exact_captured_parent_and_revision() {
    let mut pages = Pages::default();
    let original = "😀é hello\n".repeat(18000);
    let obs = observation();
    let out = publish_page(
        &mut pages,
        &obs,
        "observation",
        provider(vec![comment("comment-1", &original)], None),
    )
    .unwrap();
    let first = &out["page"]["items"][0];
    let revision = first["revision"].as_str().unwrap();
    let key = pages.texts.keys().next().unwrap().clone();
    assert_eq!(
        code(text_chunk(&mut pages, "observation", &key, 1, 64 * 1024)),
        "invalid_cursor"
    );
    let mut whole = first["body"]["text"].as_str().unwrap().to_string();
    let mut cursor = first["body"]["next_cursor"].as_str().map(str::to_string);
    while let Some(token) = cursor {
        let (_, fid) = token.split_once('.').unwrap();
        let Pending::Text { key, offset } = pages.frames[fid].pending.clone().unwrap() else {
            panic!()
        };
        assert_eq!(offset, whole.len());
        assert_eq!(pages.texts[&key].comment_id, "comment-1");
        assert_eq!(pages.texts[&key].revision, revision);
        let body = text_chunk(&mut pages, "observation", &key, offset, 64 * 1024).unwrap();
        whole.push_str(body["text"].as_str().unwrap());
        cursor = body["next_cursor"].as_str().map(str::to_string);
    }
    assert_eq!(whole, original);
}
#[test]
fn repeated_comment_edits_refuse_instead_of_replacing_captured_bytes() {
    let obs = observation();
    let mut pages = Pages::default();
    publish_page(
        &mut pages,
        &obs,
        "obs",
        provider(vec![comment("a", "old")], Some("2")),
    )
    .unwrap();
    let out = publish_page(
        &mut pages,
        &obs,
        "obs",
        provider(vec![comment("a", "old")], Some("3")),
    )
    .unwrap();
    assert_eq!(out["page"]["duplicate_observations"], 1);
    assert!(out["page"]["items"].as_array().unwrap().is_empty());
    assert_eq!(out["page"]["completeness"], "page_only");
    assert_eq!(
        code(publish_page(
            &mut pages,
            &obs,
            "obs",
            provider(vec![comment("a", "edited")], None)
        )),
        "discussion_changed"
    );
    assert_eq!(pages.texts.values().next().unwrap().text, "old");
}
#[test]
fn malformed_or_foreign_provider_identity_never_becomes_empty_success() {
    let obs = observation();
    let mut pages = Pages::default();
    publish_page(&mut pages, &obs, "obs", provider(vec![], Some("2"))).unwrap();
    let mut next = provider(vec![], None);
    next.identity.issue_id = "foreign".into();
    assert_eq!(
        code(publish_page(&mut pages, &obs, "obs", next)),
        "discussion_changed"
    );
    let mut bad = provider(vec![], None);
    bad.provider_exhausted = false;
    assert_eq!(
        code(publish_page(&mut pages, &obs, "obs", bad)),
        "upstream_unavailable"
    );
    let mut bad = provider(vec![], None);
    bad.next = Some("2".into());
    assert_eq!(
        code(publish_page(&mut pages, &obs, "obs", bad)),
        "upstream_unavailable"
    );
    assert_eq!(
        code(publish_page(
            &mut pages,
            &obs,
            "obs",
            provider(vec![comment("a", "x"), comment("a", "x")], None)
        )),
        "upstream_unavailable"
    );
}
#[test]
fn retained_capacity_and_frame_limits_are_hard_refusals() {
    let mut pages = Pages::default();
    let obs = observation();
    publish_page(
        &mut pages,
        &obs,
        "obs",
        provider(
            vec![comment("one", &"x".repeat(2 * 1024 * 1024))],
            Some("2"),
        ),
    )
    .unwrap();
    publish_page(
        &mut pages,
        &obs,
        "obs",
        provider(vec![comment("two", &"x".repeat(2 * 1024 * 1024))], None),
    )
    .unwrap();
    assert_eq!(code(encode(&pages)), "observation_capacity");
    let mut pages = Pages::default();
    for _ in 0..FRAMES {
        issue_frame(
            &mut pages,
            "obs",
            "discussion",
            Pending::Provider { after: None },
        )
        .unwrap();
    }
    assert_eq!(
        code(issue_frame(
            &mut pages,
            "obs",
            "discussion",
            Pending::Provider { after: None }
        )),
        "observation_capacity"
    );
}
#[test]
fn expiry_capacity_preserves_active_arc_reservations() {
    let cache = Cache::default();
    let mut held = vec![];
    for n in 0..8 {
        let mut o = observation();
        o.created = Instant::now() - TTL - Duration::from_secs(1);
        let arc = Arc::new(o);
        held.push(arc.clone());
        cache.insert(n.to_string(), arc).unwrap();
    }
    assert_eq!(code(cache.get("0")), "observation_expired");
    assert_eq!(
        code(cache.insert("extra".into(), Arc::new(observation()))),
        "observation_capacity"
    );
    drop(held);
    cache
        .insert("fresh".into(), Arc::new(observation()))
        .unwrap();
    assert!(cache.get("fresh").is_ok());
}
#[test]
fn exact_selectors_reject_alias_numbers_remote_and_origin_credentials() {
    assert_eq!(
        code(target("github", "007", &query(None))),
        "invalid_target"
    );
    let mut q = query(None);
    q.host = Some("peer".into());
    assert_eq!(code(target("github", "7", &q)), "unsupported");
    q = query(None);
    q.expected_origin = "https://user:secret@api.github.com".into();
    assert_eq!(code(target("github", "7", &q)), "invalid_target");
}
struct Fake {
    calls: AtomicUsize,
    epoch: AtomicUsize,
    blocked: AtomicUsize,
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
}
impl Fake {
    fn new() -> Self {
        Self {
            calls: AtomicUsize::new(0),
            epoch: AtomicUsize::new(0),
            blocked: AtomicUsize::new(0),
            entered: tokio::sync::Notify::new(),
            release: tokio::sync::Notify::new(),
        }
    }
}
#[async_trait::async_trait]
impl crate::repos::RepoLister for Fake {
    async fn list(
        &self,
    ) -> std::result::Result<Vec<crate::repos::RepoEntry>, crate::repos::RepoListError> {
        Ok(vec![])
    }
    fn issue_reader(&self) -> Option<&dyn IssueReader> {
        Some(self)
    }
}
#[async_trait::async_trait]
impl IssueReader for Fake {
    fn accounts(&self) -> Vec<IssueReadAccount> {
        vec![]
    }
    fn supports_discussion(&self) -> bool {
        true
    }
    fn comments_supported(&self, _: &IssueReadTarget) -> std::result::Result<bool, IssueReadError> {
        Ok(false)
    }
    async fn issue(&self, _: &IssueReadTarget) -> std::result::Result<Issue, IssueReadError> {
        panic!("discussion never downloads legacy issue")
    }
    async fn comments(
        &self,
        _: &IssueReadTarget,
        _: u32,
    ) -> std::result::Result<IssueCommentPage, IssueReadError> {
        panic!("no legacy comment fallback")
    }
    fn discussion_epoch(&self, _: &IssueReadTarget) -> std::result::Result<String, IssueReadError> {
        Ok(format!("epoch-{}", self.epoch.load(Ordering::SeqCst)))
    }
    async fn discussion_page(
        &self,
        _: &IssueReadTarget,
        after: Option<&str>,
    ) -> std::result::Result<DiscussionPage, IssueReadError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.blocked.load(Ordering::SeqCst) == 1 {
            self.entered.notify_one();
            self.release.notified().await;
        }
        Ok(if after.is_none() {
            provider(vec![comment("one", &"é".repeat(50000))], Some("2"))
        } else {
            provider(vec![comment("two", "final")], None)
        })
    }
}
fn state(port: Arc<Fake>) -> AppState {
    AppState::new(
        std::path::PathBuf::from("/fixture-discussion"),
        rupu_config::PricingConfig::default(),
    )
    .with_repos(Some(port))
}
#[tokio::test]
async fn replay_has_no_provider_call_and_wrong_parent_kind_owner_or_epoch_refuses() {
    let cache = Cache::default();
    let admission = Semaphore::new(2);
    let port = Arc::new(Fake::new());
    let s = state(port.clone());
    let first = read(&cache, &admission, &s, "github", "7", query(None), false)
        .await
        .unwrap()
        .0;
    let next = first["page"]["next_cursor"].as_str().unwrap().to_string();
    let text = first["page"]["items"][0]["body"]["next_cursor"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(
        code(
            read(
                &cache,
                &admission,
                &s,
                "github",
                "8",
                query(Some(text.clone())),
                true
            )
            .await
        ),
        "invalid_cursor"
    );
    assert_eq!(
        code(
            read(
                &cache,
                &admission,
                &s,
                "github",
                "7",
                query(Some(text.clone())),
                false
            )
            .await
        ),
        "invalid_cursor"
    );
    for field in ["account", "project", "origin"] {
        let mut foreign = query(Some(text.clone()));
        match field {
            "account" => foreign.account = "other".into(),
            "project" => foreign.project = "other/repo".into(),
            _ => foreign.expected_origin = "https://other.example".into(),
        }
        assert_eq!(
            code(read(&cache, &admission, &s, "github", "7", foreign, true).await),
            "invalid_cursor"
        );
    }
    let other = state(Arc::new(Fake::new()));
    assert_eq!(
        code(
            read(
                &cache,
                &admission,
                &other,
                "github",
                "7",
                query(Some(text.clone())),
                true
            )
            .await
        ),
        "invalid_cursor"
    );
    let chunk = read(
        &cache,
        &admission,
        &s,
        "github",
        "7",
        query(Some(text.clone())),
        true,
    )
    .await
    .unwrap()
    .0;
    assert_eq!(chunk["comment_id"], "one");
    assert_eq!(port.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        chunk,
        read(
            &cache,
            &admission,
            &s,
            "github",
            "7",
            query(Some(text)),
            true
        )
        .await
        .unwrap()
        .0
    );
    let page = read(
        &cache,
        &admission,
        &s,
        "github",
        "7",
        query(Some(next.clone())),
        false,
    )
    .await
    .unwrap()
    .0;
    assert_eq!(
        page,
        read(
            &cache,
            &admission,
            &s,
            "github",
            "7",
            query(Some(next.clone())),
            false
        )
        .await
        .unwrap()
        .0
    );
    assert_eq!(port.calls.load(Ordering::SeqCst), 2);
    port.epoch.store(1, Ordering::SeqCst);
    assert_eq!(
        code(
            read(
                &cache,
                &admission,
                &s,
                "github",
                "7",
                query(Some(next)),
                false
            )
            .await
        ),
        "discussion_changed"
    );
    port.epoch.store(0, Ordering::SeqCst);
    {
        let mut entries = cache.entries.lock().unwrap();
        let only = entries.values_mut().next().unwrap();
        Arc::get_mut(only).unwrap().created = Instant::now() - TTL - Duration::from_secs(1);
    }
    assert_eq!(
        code(
            read(
                &cache,
                &admission,
                &s,
                "github",
                "7",
                query(Some(first["page"]["next_cursor"].as_str().unwrap().into())),
                false
            )
            .await
        ),
        "observation_expired"
    );
}
#[tokio::test]
async fn canceled_continuation_never_advances_or_publishes_partial_frames() {
    let cache = Cache::default();
    let admission = Semaphore::new(2);
    let port = Arc::new(Fake::new());
    let s = state(port.clone());
    let first = read(&cache, &admission, &s, "github", "7", query(None), false)
        .await
        .unwrap()
        .0;
    let next = first["page"]["next_cursor"].as_str().unwrap().to_string();
    let (oid, _) = next.split_once('.').unwrap();
    let obs = cache.get(oid).unwrap();
    let before = obs.state.lock().await.clone();
    port.blocked.store(1, Ordering::SeqCst);
    let mut pending = Box::pin(read(
        &cache,
        &admission,
        &s,
        "github",
        "7",
        query(Some(next.clone())),
        false,
    ));
    tokio::select! { biased; _=port.entered.notified()=>{},result=&mut pending=>panic!("premature result {}",result.is_ok()) }
    assert_eq!(
        code(
            read(
                &cache,
                &admission,
                &s,
                "github",
                "7",
                query(Some(next.clone())),
                false
            )
            .await
        ),
        "busy"
    );
    drop(pending);
    assert_eq!(before, *obs.state.try_lock().unwrap());
    port.blocked.store(0, Ordering::SeqCst);
    let resumed = read(
        &cache,
        &admission,
        &s,
        "github",
        "7",
        query(Some(next)),
        false,
    )
    .await
    .unwrap()
    .0;
    assert_eq!(resumed["page"]["ordinal"], 1);
    assert_eq!(resumed["page"]["items"][0]["id"], "two");
}
#[tokio::test]
async fn admission_refuses_without_any_provider_call() {
    let cache = Cache::default();
    let admission = Semaphore::new(0);
    let port = Arc::new(Fake::new());
    let s = state(port.clone());
    assert_eq!(
        code(read(&cache, &admission, &s, "github", "7", query(None), false).await),
        "busy"
    );
    assert_eq!(port.calls.load(Ordering::SeqCst), 0);
}

#[test]
fn nonadjacent_provider_cursor_cycle_is_refused_without_frontier_publication() {
    let obs = observation();
    let mut pages = Pages::default();
    publish_page(&mut pages, &obs, "obs", provider(vec![], Some("A"))).unwrap();
    publish_page(&mut pages, &obs, "obs", provider(vec![], Some("B"))).unwrap();
    let before = encode(&pages).unwrap();
    let mut draft: Pages = serde_json::from_slice(&before).unwrap();
    assert_eq!(
        code(publish_page(
            &mut draft,
            &obs,
            "obs",
            provider(vec![], Some("A"))
        )),
        "upstream_unavailable"
    );
    assert_eq!(encode(&pages).unwrap(), before);
    assert_eq!(pages.ordinal, 2);
}
