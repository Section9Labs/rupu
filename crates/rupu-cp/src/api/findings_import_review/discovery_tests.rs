use super::*;
use std::fs;
use std::os::unix::fs::symlink;

fn fixture() -> (tempfile::TempDir, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let canonical = fs::canonicalize(temp.path()).unwrap();
    (temp, canonical)
}

fn register(global: &Path, id: &str, workspace: &Path) {
    fs::create_dir_all(global.join("workspaces")).unwrap();
    let record = rupu_workspace::Workspace {
        id: id.to_owned(),
        path: workspace.to_str().unwrap().to_owned(),
        repo_remote: None,
        initial_branch: None,
        created_at: "2026-01-01T00:00:00Z".into(),
        last_run_at: None,
    };
    fs::write(
        global.join("workspaces").join(format!("{id}.toml")),
        toml::to_string(&record).unwrap(),
    )
    .unwrap();
}

fn finding(id: &str) -> serde_json::Value {
    serde_json::json!({"id":id,"scope":"repo","summary":"recorded summary","severity":"high",
        "evidence":{"rationale":"fixture"},"declared_by":{"run_id":"run_test","model":"fixture","surface":"agent"},
        "declared_at":"2026-01-01T00:00:00Z","future_field":{"retain":true}})
}

fn ledger(workspace: &Path, target: &str, id: &str) -> PathBuf {
    let paths = CoveragePaths::new(workspace, target);
    paths.ensure_dir().unwrap();
    fs::write(&paths.findings, format!("{}\n", finding(id))).unwrap();
    paths.findings
}

#[test]
fn captures_exact_bytes_and_deduplicates_same_canonical_report() {
    let (_temp, root) = fixture();
    let path = root.join("report.md");
    let bytes = b"# Report\r\nexact bytes\n";
    fs::write(&path, bytes).unwrap();
    let captured = discover_reports(&[path.clone(), path.clone()], None).unwrap();
    assert_eq!(captured.len(), 1);
    assert_eq!(captured[0].original_path, path);
    assert_eq!(captured[0].canonical_path, path);
    assert_eq!(captured[0].bytes, bytes);
    assert_eq!(captured[0].sha256, format!("{:x}", Sha256::digest(bytes)));
    let roundtrip: FileIdentity =
        serde_json::from_slice(&serde_json::to_vec(&captured[0].identity).unwrap()).unwrap();
    assert_eq!(roundtrip, captured[0].identity);
}

#[test]
fn refuses_symlink_parents_final_links_and_hardlinked_reports() {
    let (_temp, root) = fixture();
    let dir = root.join("real");
    fs::create_dir(&dir).unwrap();
    let path = dir.join("report.md");
    fs::write(&path, "report").unwrap();
    symlink(&dir, root.join("linked")).unwrap();
    symlink(&path, root.join("linked.md")).unwrap();
    assert!(read_regular_no_follow(&root.join("linked/report.md"), 100).is_err());
    assert!(read_regular_no_follow(&root.join("linked.md"), 100).is_err());
    fs::hard_link(&path, root.join("hard.md")).unwrap();
    assert_eq!(
        read_regular_no_follow(&path, 100).unwrap_err().code,
        "alias"
    );
}

#[test]
fn selection_and_held_file_size_limits_fail_explicitly() {
    let (_temp, root) = fixture();
    let path = root.join("report.md");
    fs::write(&path, b"12345").unwrap();
    assert_eq!(read_regular_no_follow(&path, 4).unwrap_err().code, "limit");
    assert!(read_regular_no_follow(&root, 100).is_err());
    assert!(read_regular_no_follow(Path::new("relative.md"), 100).is_err());
    assert!(read_regular_no_follow(&root.join("../outside.md"), 100).is_err());
    assert!(discover_reports(&[], None).is_err());
    assert!(discover_reports(std::slice::from_ref(&path), Some(&root)).is_err());
    assert!(discover_reports(&vec![path; MAX_REPORTS + 1], None).is_err());
}

#[test]
fn directory_selection_is_recursive_sorted_and_markdown_only() {
    let (_temp, root) = fixture();
    fs::create_dir(root.join("child")).unwrap();
    fs::write(root.join("child/z.MD"), "nested").unwrap();
    fs::write(root.join("a.md"), "first").unwrap();
    fs::write(root.join("ignored.txt"), "not selected").unwrap();
    let files = discover_reports(&[], Some(&root)).unwrap();
    assert_eq!(files.len(), 2);
    assert_eq!(files[0].original_path, root.join("a.md"));
    assert_eq!(files[1].original_path, root.join("child/z.MD"));
    symlink(root.join("ignored.txt"), root.join("link.txt")).unwrap();
    assert_eq!(
        discover_reports(&[], Some(&root)).unwrap_err().code,
        "symlink"
    );
}

#[test]
fn directory_count_and_total_report_bytes_are_bounded() {
    let (_temp, root) = fixture();
    for n in 0..=MAX_REPORTS {
        fs::write(root.join(format!("{n:03}.md")), "small").unwrap();
    }
    assert_eq!(
        discover_reports(&[], Some(&root)).unwrap_err().code,
        "limit"
    );
    let (_temp2, large) = fixture();
    let mut paths = Vec::new();
    for n in 0..5 {
        let path = large.join(format!("{n}.md"));
        File::create(&path)
            .unwrap()
            .set_len(MAX_REPORT_BYTES as u64)
            .unwrap();
        paths.push(path);
    }
    assert_eq!(discover_reports(&paths, None).unwrap_err().code, "limit");
}

#[test]
fn visit_budget_counts_unselected_entries_too() {
    let (_temp, root) = fixture();
    for n in 0..=MAX_VISITS {
        fs::write(root.join(format!("{n:05}.txt")), "").unwrap();
    }
    assert_eq!(
        discover_reports(&[], Some(&root)).unwrap_err().code,
        "limit"
    );
}

#[test]
fn complete_scan_retains_qualified_owners_duplicate_ids_and_raw_evidence() {
    let (_temp, root) = fixture();
    let a = root.join("a");
    let b = root.join("b");
    fs::create_dir(&a).unwrap();
    fs::create_dir(&b).unwrap();
    register(&root, "ws_a", &a);
    register(&root, "ws_b", &b);
    let a_path = ledger(&a, "target_a", "same_id");
    ledger(&b, "target_b", "same_id");
    let captured = discover_ledgers(&root).unwrap();
    assert_eq!(captured.len(), 2);
    assert_eq!(
        (&*captured[0].ws_id, &*captured[0].target_id),
        ("ws_a", "target_a")
    );
    assert_eq!(captured[0].paths().findings, a_path);
    assert_eq!(captured[0].records[0].id, captured[1].records[0].id);
    assert_eq!(captured[0].raw_records[0]["future_field"]["retain"], true);
    assert_eq!(
        captured[0].ledger.sha256,
        format!("{:x}", Sha256::digest(fs::read(a_path).unwrap()))
    );
    assert_eq!(captured[0].workspace_path, a);
    assert_eq!(
        captured[0].registry_record.original_path,
        root.join("workspaces/ws_a.toml")
    );
}

#[test]
fn workspace_aliases_and_malformed_registry_cannot_silently_disappear() {
    let (_temp, root) = fixture();
    let workspace = root.join("repo");
    fs::create_dir(&workspace).unwrap();
    register(&root, "ws_a", &workspace);
    register(&root, "ws_b", &workspace);
    assert_eq!(discover_ledgers(&root).unwrap_err().code, "alias");
    fs::remove_file(root.join("workspaces/ws_b.toml")).unwrap();
    fs::write(root.join("workspaces/ws_bad.toml"), "not = [valid").unwrap();
    assert_eq!(
        discover_ledgers(&root).unwrap_err().code,
        "invalid_registry"
    );
}

#[test]
fn absent_ledger_is_empty_but_new_ledger_appears_on_rediscovery() {
    let (_temp, root) = fixture();
    let workspace = root.join("repo");
    fs::create_dir(&workspace).unwrap();
    register(&root, "ws_a", &workspace);
    let paths = CoveragePaths::new(&workspace, "empty");
    paths.ensure_dir().unwrap();
    assert!(discover_ledgers(&root).unwrap().is_empty());
    ledger(&workspace, "empty", "new_finding");
    assert_eq!(
        discover_ledgers(&root).unwrap()[0].records[0].id,
        "new_finding"
    );
}

#[test]
fn malformed_nonregular_symlinked_and_hardlinked_ledgers_abort_inventory() {
    let (_temp, root) = fixture();
    let workspace = root.join("repo");
    fs::create_dir(&workspace).unwrap();
    register(&root, "ws_a", &workspace);
    let path = ledger(&workspace, "target", "one");
    fs::write(&path, format!("{}\n{{torn\n", finding("one"))).unwrap();
    assert_eq!(discover_ledgers(&root).unwrap_err().code, "invalid_ledger");
    fs::remove_file(&path).unwrap();
    fs::create_dir(&path).unwrap();
    assert_eq!(discover_ledgers(&root).unwrap_err().code, "not_regular");
    fs::remove_dir(&path).unwrap();
    let other = root.join("other");
    fs::write(&other, "").unwrap();
    symlink(&other, &path).unwrap();
    assert!(discover_ledgers(&root).is_err());
    fs::remove_file(&path).unwrap();
    fs::hard_link(&other, &path).unwrap();
    assert_eq!(discover_ledgers(&root).unwrap_err().code, "alias");
}

#[test]
fn symlinked_coverage_ancestors_are_not_followed() {
    let (_temp, root) = fixture();
    let workspace = root.join("repo");
    fs::create_dir(&workspace).unwrap();
    register(&root, "ws_a", &workspace);
    let target = root.join("elsewhere");
    fs::create_dir(&target).unwrap();
    symlink(&target, workspace.join(".rupu")).unwrap();
    assert!(discover_ledgers(&root).is_err());
}

#[test]
fn held_file_cannot_be_relabelled_after_path_replacement() {
    let (_temp, root) = fixture();
    let path = root.join("report.md");
    fs::write(&path, "original").unwrap();
    let held = open_absolute(&path, false).unwrap();
    fs::rename(&path, root.join("moved.md")).unwrap();
    fs::write(&path, "replacement").unwrap();
    assert_eq!(capture_file(held, &path, 100).unwrap_err().code, "changed");
}

#[test]
fn unreadable_as_regular_registry_entry_does_not_become_an_empty_catalog() {
    let (_temp, root) = fixture();
    fs::create_dir_all(root.join("workspaces/ws_bad.toml")).unwrap();
    assert_eq!(discover_ledgers(&root).unwrap_err().code, "not_regular");
}

#[test]
fn oversized_registry_and_ledger_are_refused_before_reading() {
    let (_temp, root) = fixture();
    let workspace = root.join("repo");
    fs::create_dir(&workspace).unwrap();
    register(&root, "ws_a", &workspace);
    let path = ledger(&workspace, "target", "one");
    File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_len(MAX_LEDGER_BYTES as u64 + 1)
        .unwrap();
    assert_eq!(discover_ledgers(&root).unwrap_err().code, "limit");
    fs::remove_file(path).unwrap();
    File::options()
        .write(true)
        .open(root.join("workspaces/ws_a.toml"))
        .unwrap()
        .set_len(MAX_REGISTRY_BYTES as u64 + 1)
        .unwrap();
    assert_eq!(discover_ledgers(&root).unwrap_err().code, "limit");
}
