//! End-to-end: populate a coverage target on disk, then run
//! `rupu coverage audit <id> --json` through the CLI entrypoint.
//!
//! This test mutates process-global state (cwd). Hold `ENV_LOCK` for
//! the whole body of the test to serialise any future cwd-mutating
//! tests that land in this binary.

use chrono::Utc;
use rupu_coverage::{
    flatten, write_snapshot, AssertionStatus, Attribution, CatalogMode, ConcernAssertion,
    ConcernsBlock, ConcernsEntry, CoveragePaths, Evidence, FileTouchEvent, IncludeDirective,
    Surface,
};
use tokio::sync::Mutex;

static ENV_LOCK: Mutex<()> = Mutex::const_new(());

#[tokio::test]
async fn coverage_audit_cli_runs_on_populated_target() {
    let _guard = ENV_LOCK.lock().await;

    let tmp = tempfile::TempDir::new().unwrap();
    let paths = CoveragePaths::new(tmp.path(), "tgt");
    paths.ensure_dir().unwrap();

    // Build a minimal stride catalog snapshot.
    let cat = flatten(&ConcernsBlock {
        entries: vec![ConcernsEntry::Include(IncludeDirective {
            include: "stride".to_string(),
            overrides: vec![],
            mode: CatalogMode::Auto,
            filter: None,
        })],
    })
    .unwrap();
    write_snapshot(&cat, &paths.catalog).unwrap();

    // Write one file-touch event.
    let attribution = Attribution {
        run_id: "r".into(),
        model: "m".into(),
        surface: Surface::Workflow,
    };
    let touch = FileTouchEvent::Read {
        path: "src/auth/login.rs".into(),
        line_range: [1, 80],
        tool: "read_file".into(),
        attribution: attribution.clone(),
        at: Utc::now(),
    };
    std::fs::write(&paths.files, serde_json::to_string(&touch).unwrap() + "\n").unwrap();

    // Write one concern assertion for stride:spoofing.
    let assertion = ConcernAssertion {
        concern_id: "stride:spoofing".into(),
        file_path: "src/auth/login.rs".into(),
        status: AssertionStatus::Clean,
        evidence: Evidence {
            summary: "ok".into(),
            line_ranges: vec![],
            finding_ids: vec![],
        },
        declared_by: attribution,
        declared_at: Utc::now(),
    };
    std::fs::write(
        &paths.concerns,
        serde_json::to_string(&assertion).unwrap() + "\n",
    )
    .unwrap();

    // Drive the real CLI dispatcher with cwd set to the tempdir so that
    // workspace discovery resolves `.rupu/coverage/tgt/`.
    let prev = std::env::current_dir().unwrap();
    std::env::set_current_dir(tmp.path()).unwrap();
    let code = rupu_cli::run(vec![
        "rupu".into(),
        "coverage".into(),
        "audit".into(),
        "tgt".into(),
        "--json".into(),
    ])
    .await;
    std::env::set_current_dir(prev).unwrap();

    assert_eq!(
        format!("{code:?}"),
        format!("{:?}", std::process::ExitCode::from(0)),
    );
}
