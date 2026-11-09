use super::*;
use crate::issue_reader::{IssueReadAccount, IssueReadTarget};
use rupu_scm::{issue_discovery::DiscoveryRow, IssueTracker};
use std::sync::atomic::{AtomicUsize, Ordering};
fn target() -> IssueDiscoveryTarget {
    IssueDiscoveryTarget {
        tracker: IssueTracker::Github,
        project: "org/repo".into(),
        account: "work".into(),
        origin: "https://api.github.com".into(),
    }
}
fn filters() -> Filters {
    Filters {
        state: DiscoveryState::Open,
        title_query: None,
    }
}
fn row() -> DiscoveryRow {
    DiscoveryRow {
        number: u64::MAX.to_string(),
        title: "current".into(),
        state: "open".into(),
        updated_at: None,
        url: None,
        title_truncated: false,
    }
}
fn provider() -> DiscoveryPage {
    DiscoveryPage {
        project: "org/repo".into(),
        provider_project_id: "repo-1".into(),
        connector_epoch: "epoch".into(),
        rows: vec![row()],
        next: Some("2".into()),
        end_observed: false,
        truncated: false,
        search_mode: "none".into(),
    }
}
fn cursor() -> Cursor {
    Cursor {
        schema: 1,
        instance: "owner".into(),
        target: target(),
        filters: filters(),
        epoch: "epoch".into(),
        after: "2".into(),
        provider_project_id: "repo-1".into(),
        issued_at: 1000,
        ordinal: 1,
    }
}
#[test]
fn signed_continuation_binds_account_origin_project_tracker_query_instance_and_epoch() {
    let encoded = encode(&cursor()).unwrap();
    assert_eq!(
        decode(&encoded, "owner", &target(), &filters(), "epoch", 1001)
            .unwrap()
            .after,
        "2"
    );
    let mut alternatives = vec![target(); 4];
    alternatives[0].account = "other".into();
    alternatives[1].origin = "https://other.example".into();
    alternatives[2].project = "other/repo".into();
    alternatives[3].tracker = IssueTracker::Gitlab;
    for t in alternatives {
        assert_eq!(
            decode(&encoded, "owner", &t, &filters(), "epoch", 1001)
                .unwrap_err()
                .1,
            "selection_changed"
        );
    }
    assert!(decode(&encoded, "other", &target(), &filters(), "epoch", 1001).is_err());
    assert!(decode(&encoded, "owner", &target(), &filters(), "new-epoch", 1001).is_err());
    let mut f = filters();
    f.state = DiscoveryState::All;
    assert!(decode(&encoded, "owner", &target(), &f, "epoch", 1001).is_err());
    f = filters();
    f.title_query = Some("literal".into());
    assert!(decode(&encoded, "owner", &target(), &f, "epoch", 1001).is_err());
}
#[test]
fn continuation_rejects_tamper_expiry_future_time_and_size_without_provider_work() {
    let value = encode(&cursor()).unwrap();
    assert_eq!(
        decode(&value, "owner", &target(), &filters(), "epoch", 1600)
            .unwrap_err()
            .1,
        "cursor_expired"
    );
    assert!(decode(&value, "owner", &target(), &filters(), "epoch", 999).is_err());
    let (p, s) = value.split_once('.').unwrap();
    let mut bytes = URL_SAFE_NO_PAD.decode(p).unwrap();
    bytes[0] ^= 1;
    assert_eq!(
        decode(
            &format!("{}.{}", URL_SAFE_NO_PAD.encode(bytes), s),
            "owner",
            &target(),
            &filters(),
            "epoch",
            1001
        )
        .unwrap_err()
        .1,
        "invalid_cursor"
    );
    assert!(decode(
        &"x".repeat(CURSOR_BYTES + 1),
        "owner",
        &target(),
        &filters(),
        "epoch",
        1001
    )
    .is_err());
}
#[test]
fn row_bounds_and_decimal_identity_never_silently_clip_or_relabel() {
    assert!(validate_page(&provider(), &filters()).is_ok());
    let mut p = provider();
    p.rows[0].number = "01".into();
    assert!(validate_page(&p, &filters()).is_err());
    p = provider();
    p.rows[0].title = "x".repeat(4097);
    assert!(validate_page(&p, &filters()).is_err());
    p = provider();
    p.rows.push(row());
    assert!(validate_page(&p, &filters()).is_err());
    p = provider();
    p.rows[0].url = Some("https://user:secret@example.com".into());
    assert!(validate_page(&p, &filters()).is_err());
    p = provider();
    p.rows[0].state = "closed".into();
    assert!(validate_page(&p, &filters()).is_err());
    p = provider();
    p.end_observed = true;
    assert!(validate_page(&p, &filters()).is_err());
    p = provider();
    p.rows.clear();
    assert!(validate_page(&p, &filters()).is_ok());
    p = provider();
    p.rows = (1..=26)
        .map(|n| {
            let mut r = row();
            r.number = n.to_string();
            r
        })
        .collect();
    assert!(validate_page(&p, &filters()).is_err());
}
#[test]
fn query_rejects_remote_host_unknown_fields_secret_origin_and_blank_search() {
    let q = json!({"host":"local","project":"org/repo","account":"work","expected_origin":"https://api.github.com","state":"open"});
    assert!(selection(
        "github",
        &serde_json::from_value::<DiscoveryQuery>(q.clone()).unwrap()
    )
    .is_ok());
    for (field, value) in [
        ("host", "remote"),
        ("expected_origin", "https://u:secret@example.com"),
        ("title_query", " "),
    ] {
        let mut bad = q.clone();
        bad[field] = json!(value);
        assert!(selection("github", &serde_json::from_value(bad).unwrap()).is_err());
    }
    let mut bad = q;
    bad["workspace"] = json!("inferred");
    assert!(serde_json::from_value::<DiscoveryQuery>(bad).is_err());
}
struct Fake {
    calls: AtomicUsize,
    mutate_epoch: bool,
    malformed_row: bool,
}
#[async_trait::async_trait]
impl IssueReader for Fake {
    fn accounts(&self) -> Vec<IssueReadAccount> {
        vec![]
    }
    fn supports_discovery(&self) -> bool {
        true
    }
    fn discovery_epoch(
        &self,
        _: &IssueDiscoveryTarget,
    ) -> std::result::Result<String, IssueReadError> {
        Ok(
            if self.mutate_epoch && self.calls.load(Ordering::SeqCst) > 0 {
                "changed"
            } else {
                "epoch"
            }
            .into(),
        )
    }
    async fn discovery_page(
        &self,
        _: &IssueDiscoveryTarget,
        request: &DiscoveryRequest,
    ) -> std::result::Result<DiscoveryPage, IssueReadError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let mut p = provider();
        if request.after.is_some() {
            p.next = None;
            p.end_observed = true;
        }
        if self.malformed_row {
            p.rows[0].number = "0".into();
        }
        Ok(p)
    }
    fn comments_supported(&self, _: &IssueReadTarget) -> std::result::Result<bool, IssueReadError> {
        unreachable!()
    }
    async fn issue(
        &self,
        _: &IssueReadTarget,
    ) -> std::result::Result<rupu_scm::Issue, IssueReadError> {
        unreachable!()
    }
    async fn comments(
        &self,
        _: &IssueReadTarget,
        _: u32,
    ) -> std::result::Result<rupu_scm::operator_issues::IssueCommentPage, IssueReadError> {
        unreachable!()
    }
}
#[tokio::test]
async fn live_page_continuation_echoes_exact_selection_and_never_invents_total() {
    let port = Fake {
        calls: AtomicUsize::new(0),
        mutate_epoch: false,
        malformed_row: false,
    };
    let first = read(&port, "owner", target(), filters(), None)
        .await
        .unwrap()
        .0;
    assert_eq!(first["identity"]["project"], "org/repo");
    assert_eq!(first["rows"][0]["number"], u64::MAX.to_string());
    assert!(first["pagination"]["total"].is_null());
    let second = read(
        &port,
        "owner",
        target(),
        filters(),
        first["pagination"]["next_cursor"].as_str(),
    )
    .await
    .unwrap()
    .0;
    assert_eq!(first["expires_at"], second["expires_at"]);
    assert!(second["pagination"]["end_observed"].as_bool().unwrap());
    assert_eq!(port.calls.load(Ordering::SeqCst), 2);
    let mut other = target();
    other.account = "other".into();
    assert!(read(
        &port,
        "owner",
        other,
        filters(),
        first["pagination"]["next_cursor"].as_str()
    )
    .await
    .is_err());
    assert_eq!(port.calls.load(Ordering::SeqCst), 2);
}
#[tokio::test]
async fn changed_epoch_or_malformed_adapter_row_never_publishes() {
    for (mutate_epoch, malformed_row) in [(true, false), (false, true)] {
        let port = Fake {
            calls: AtomicUsize::new(0),
            mutate_epoch,
            malformed_row,
        };
        assert!(read(&port, "owner", target(), filters(), None)
            .await
            .is_err());
        assert_eq!(port.calls.load(Ordering::SeqCst), 1);
    }
}
#[tokio::test]
async fn same_project_path_with_replaced_provider_object_refuses_continuation() {
    let port = Fake {
        calls: AtomicUsize::new(0),
        mutate_epoch: false,
        malformed_row: false,
    };
    let mut old = cursor();
    old.issued_at = chrono::Utc::now().timestamp();
    old.provider_project_id = "deleted-repo".into();
    let token = encode(&old).unwrap();
    let error = read(&port, "owner", target(), filters(), Some(&token))
        .await
        .unwrap_err();
    assert_eq!(error.1, "selection_changed");
    assert_eq!(port.calls.load(Ordering::SeqCst), 1);
}
