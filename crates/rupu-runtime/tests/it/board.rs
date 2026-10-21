use chrono::Utc;
use rupu_fleet::{Board, ClaimOutcome, ClaimState, Directive};
use rupu_runtime::services::board::board_view;
use std::time::Duration;

#[test]
fn board_view_folds_claims_and_directives() {
    let tmp = tempfile::tempdir().unwrap();
    let board = Board::new(tmp.path());
    let ClaimOutcome::Granted(held) = board.claim("a", "lynx#1", Duration::from_secs(60)).unwrap()
    else {
        panic!("granted")
    };
    let ClaimOutcome::Granted(done) = board.claim("b", "lynx#2", Duration::from_secs(60)).unwrap()
    else {
        panic!("granted")
    };
    drop(done);
    let mk = |body: &str| Directive {
        id: String::new(),
        author: "lead".into(),
        ts: Utc::now().to_rfc3339(),
        body: body.into(),
        addressed_to: Some("role:recon".into()),
    };
    let keep = board.put_directive(&mk("stay in scope")).unwrap();
    let gone = board.put_directive(&mk("skip staging")).unwrap();
    board.retract_directive(&gone).unwrap();

    let view = board_view(tmp.path(), Utc::now()).unwrap();
    assert!(view.journal);
    assert_eq!(view.claims.len(), 2);
    assert_eq!(view.claims[0].key, "a");
    assert_eq!(view.claims[0].state, ClaimState::Held);
    assert_eq!(view.claims[1].state, ClaimState::Released);
    assert_eq!(view.directives.len(), 2);
    assert_eq!(view.directives[0].id, keep);
    assert!(!view.directives[0].retracted);
    assert!(view.directives[1].retracted);
    assert!(view.directives[1].retracted_at.is_some());
    assert_eq!(
        view.directives[1].addressed_to.as_deref(),
        Some("role:recon")
    );
    drop(held);
}

#[test]
fn board_view_of_an_empty_space_is_empty_not_an_error() {
    let tmp = tempfile::tempdir().unwrap();
    let view = board_view(tmp.path(), Utc::now()).unwrap();
    assert!(view.claims.is_empty());
    assert!(view.directives.is_empty());
    assert!(!view.journal);
}
