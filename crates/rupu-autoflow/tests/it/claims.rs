//! Claim repair, the manual-run claim check and the doctor.

use crate::owned_claim;
use rupu_autoflow::claims::ensure_manual_run_can_take_claim;
use rupu_autoflow::{doctor_report, paths, refusal_of, repair_claim, wakes_report, Refusal};
use rupu_workspace::{AutoflowClaimStore, ClaimStatus};

const ISSUE: &str = "github:Section9Labs/rupu/issues/42";

#[test]
fn a_claim_another_cycle_owns_is_a_typed_conflict() {
    let tmp = tempfile::tempdir().unwrap();
    let store = AutoflowClaimStore {
        root: tmp.path().join("claims"),
    };
    store
        .save(&owned_claim(ISSUE, ClaimStatus::AwaitExternal))
        .unwrap();
    let err = ensure_manual_run_can_take_claim(&store, ISSUE).unwrap_err();
    assert!(
        matches!(refusal_of(&err), Some(Refusal::Conflict(_))),
        "{err:#}"
    );
}

#[test]
fn a_dry_run_repair_reports_the_release_and_changes_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let global = tmp.path();
    let store = AutoflowClaimStore {
        root: paths::autoflow_claims_dir(global),
    };
    store
        .save(&owned_claim(ISSUE, ClaimStatus::Blocked))
        .unwrap();

    let dry = repair_claim(global, ISSUE, true, false, true).unwrap();
    assert!(dry.dry_run);
    assert_eq!(dry.repairs, vec![format!("would release claim {ISSUE}")]);
    assert!(
        store.load(ISSUE).unwrap().is_some(),
        "a dry run kept the claim"
    );

    let done = repair_claim(global, ISSUE, true, false, false).unwrap();
    assert_eq!(done.repairs, vec![format!("released claim {ISSUE}")]);
    assert!(
        store.load(ISSUE).unwrap().is_none(),
        "the real run released it"
    );
}

#[test]
fn repair_refusals_are_typed() {
    let tmp = tempfile::tempdir().unwrap();
    let both = repair_claim(tmp.path(), ISSUE, true, true, true).unwrap_err();
    assert_eq!(
        refusal_of(&both),
        Some(&Refusal::Invalid(
            "repair accepts at most one of `--release` or `--requeue`".into()
        ))
    );
    let missing = repair_claim(tmp.path(), ISSUE, false, false, true).unwrap_err();
    assert_eq!(
        refusal_of(&missing),
        Some(&Refusal::NotFound(format!(
            "no autoflow claim for `{ISSUE}`"
        )))
    );
    // The library reads the full form only; the CLI resolves short forms.
    for bad in ["not an issue", "42", "#42"] {
        let err = repair_claim(tmp.path(), bad, false, false, true).unwrap_err();
        assert!(
            matches!(refusal_of(&err), Some(Refusal::Invalid(_))),
            "{bad}: {err:#}"
        );
    }
}

#[test]
fn doctor_reports_a_claim_on_an_untracked_repo() {
    let tmp = tempfile::tempdir().unwrap();
    let global = tmp.path();
    AutoflowClaimStore {
        root: paths::autoflow_claims_dir(global),
    }
    .save(&owned_claim(ISSUE, ClaimStatus::AwaitExternal))
    .unwrap();
    let report = serde_json::to_value(doctor_report(global, None).unwrap()).unwrap();
    assert_eq!(report["kind"], "autoflow_doctor");
    assert_eq!(report["ok"], false);
    assert_eq!(report["rows"][0]["scope"], ISSUE);
    assert_eq!(report["rows"][0]["problem"], "missing_repo_binding");
    assert_eq!(
        serde_json::to_value(wakes_report(global, None).unwrap()).unwrap(),
        serde_json::json!({"kind": "autoflow_wakes", "version": 1, "rows": []})
    );
}

#[test]
fn a_bad_repo_filter_is_invalid() {
    let tmp = tempfile::tempdir().unwrap();
    let err = doctor_report(tmp.path(), Some("not a repo")).unwrap_err();
    assert!(
        matches!(refusal_of(&err), Some(Refusal::Invalid(_))),
        "{err:#}"
    );
}
