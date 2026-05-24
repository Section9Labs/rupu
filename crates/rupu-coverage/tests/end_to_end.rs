use rupu_coverage::{
    coverage_mark, flatten, read_snapshot, target_id, write_snapshot, AssertionStatus,
    Attribution, ConcernAssertion, ConcernsBlock, ConcernsEntry, CoveragePaths,
    CoverageWriterHandle, Evidence, FileTouchEvent, IncludeDirective, Surface,
};
use chrono::Utc;

#[tokio::test]
async fn end_to_end_workflow_with_stride_catalog() {
    let tmp = tempfile::TempDir::new().unwrap();
    let workspace = tmp.path().to_path_buf();

    // 1. Construct a ConcernsBlock with stride
    let block = ConcernsBlock {
        entries: vec![ConcernsEntry::Include(IncludeDirective {
            include: "stride".to_string(),
            overrides: vec![],
        })],
    };
    let catalog = flatten(&block).unwrap();
    assert_eq!(catalog.concerns.len(), 6);

    // 2. Establish target paths + write snapshot
    let target = target_id(&workspace, "security-review");
    let paths = CoveragePaths::new(&workspace, &target);
    paths.ensure_dir().unwrap();
    write_snapshot(&catalog, &paths.catalog).unwrap();

    // 3. Spawn writer and emit a synthetic file touch
    let handle = CoverageWriterHandle::spawn(paths.clone()).unwrap();
    let attribution = Attribution {
        run_id: "run_e2e_test".to_string(),
        model: "mock".to_string(),
        surface: Surface::Workflow,
    };
    handle
        .writer
        .record_file_touch(FileTouchEvent::Read {
            path: "src/auth/login.rs".to_string(),
            line_range: [1, 80],
            tool: "read_file".to_string(),
            attribution: attribution.clone(),
            at: Utc::now(),
        })
        .await;
    // Shutdown to flush
    handle.shutdown().await;

    // 4. Call coverage_mark
    let out = coverage_mark(
        &paths,
        &catalog,
        attribution,
        rupu_coverage::CoverageMarkInput {
            concern_id: "stride:spoofing".to_string(),
            file_path: "src/auth/login.rs".to_string(),
            status: AssertionStatus::Clean,
            evidence: Evidence {
                summary: "Token check covers all entry points.".to_string(),
                line_ranges: vec![[1, 80]],
                finding_ids: vec![],
            },
        },
    )
    .await
    .unwrap();
    assert!(out.ok);

    // 5. Verify ledger artifacts exist and look right
    assert!(paths.catalog.exists());
    assert!(paths.files.exists());
    assert!(paths.concerns.exists());

    let snapshot = read_snapshot(&paths.catalog).unwrap();
    assert_eq!(snapshot.concerns.len(), 6);

    let touches: Vec<_> = std::fs::read_to_string(&paths.files)
        .unwrap()
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| serde_json::from_str::<FileTouchEvent>(l).unwrap())
        .collect();
    assert_eq!(touches.len(), 1);

    let assertions: Vec<_> = std::fs::read_to_string(&paths.concerns)
        .unwrap()
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| serde_json::from_str::<ConcernAssertion>(l).unwrap())
        .collect();
    assert_eq!(assertions.len(), 1);
    assert_eq!(assertions[0].concern_id, "stride:spoofing");
    assert_eq!(assertions[0].status, AssertionStatus::Clean);
}
