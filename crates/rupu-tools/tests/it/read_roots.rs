use rupu_coverage::{CoveragePaths, CoverageWriterHandle};
use rupu_tools::{Tool, ToolContext};
use serde_json::json;
use std::fs;

fn setup() -> (
    tempfile::TempDir,
    tempfile::TempDir,
    std::path::PathBuf,
    ToolContext,
) {
    let ws = tempfile::tempdir().unwrap();
    let skills = tempfile::tempdir().unwrap();
    let root = skills.path().join("pdf");
    fs::create_dir_all(root.join("references")).unwrap();
    fs::write(root.join("references/REF.md"), "needle in a reference\n").unwrap();
    let root = root.canonicalize().unwrap();
    let mut ctx = ToolContext::in_workspace(ws.path());
    ctx.workspace.read_roots = vec![root.clone()];
    (ws, skills, root, ctx)
}

#[tokio::test]
async fn read_file_reads_under_a_read_root_but_write_file_refuses() {
    let (_ws, _s, root, ctx) = setup();
    let path = root.join("references/REF.md");
    let out = rupu_tools::ReadFileTool
        .invoke(json!({"path": path}), &ctx)
        .await
        .unwrap();
    assert_eq!(out.error, None, "{:?}", out.error);
    assert!(out.stdout.contains("needle in a reference"));
    let w = rupu_tools::WriteFileTool
        .invoke(json!({"path": path, "content": "x"}), &ctx)
        .await
        .unwrap();
    assert!(w.error.unwrap().contains("escapes workspace"));
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        "needle in a reference\n"
    );
}

#[tokio::test]
async fn paths_outside_every_root_stay_refused() {
    let (_ws, _s, _root, ctx) = setup();
    let other = tempfile::tempdir().unwrap();
    fs::write(other.path().join("secret"), "s").unwrap();
    let out = rupu_tools::ReadFileTool
        .invoke(json!({"path": other.path().join("secret")}), &ctx)
        .await
        .unwrap();
    assert!(out.error.is_some());
}

#[tokio::test]
async fn grep_and_glob_search_a_read_root() {
    let (_ws, _s, root, ctx) = setup();
    let g = rupu_tools::GrepTool
        .invoke(json!({"pattern": "needle", "path": root}), &ctx)
        .await
        .unwrap();
    assert_eq!(g.error, None);
    assert!(g.stdout.contains("REF.md"), "{}", g.stdout);
    let pattern = format!("{}/**/*.md", root.display());
    let gl = rupu_tools::GlobTool
        .invoke(json!({"pattern": pattern}), &ctx)
        .await
        .unwrap();
    assert!(
        gl.stdout
            .contains(&root.join("references/REF.md").display().to_string()),
        "{}",
        gl.stdout
    );
}

/// Reads under a read root are not coverage of the target, so they write no
/// `files` touch; the workspace read in the same run proves the writer is
/// live.
#[tokio::test]
async fn reads_under_a_read_root_write_no_coverage_touch() {
    let (ws, _s, root, mut ctx) = setup();
    fs::write(ws.path().join("target.txt"), "needle in the target\n").unwrap();
    let ledger = tempfile::tempdir().unwrap();
    let paths = CoveragePaths::new(ledger.path(), "test-target");
    let handle = CoverageWriterHandle::spawn(paths.clone()).unwrap();
    {
        ctx.services.coverage_writer = Some(handle.writer.clone());
        let ref_md = root.join("references/REF.md");
        let pattern = format!("{}/**/*.md", root.display());
        let calls = [
            (
                &rupu_tools::ReadFileTool as &dyn Tool,
                json!({"path": ref_md}),
            ),
            (
                &rupu_tools::GrepTool,
                json!({"pattern": "needle", "path": root}),
            ),
            (&rupu_tools::GlobTool, json!({"pattern": pattern})),
            (&rupu_tools::ReadFileTool, json!({"path": "target.txt"})),
        ];
        for (tool, input) in calls {
            let out = tool.invoke(input, &ctx).await.unwrap();
            assert_eq!(out.error, None, "{}", tool.descriptor().name);
        }
        drop(ctx);
    }
    handle.shutdown().await;
    let touched: Vec<String> = fs::read_to_string(&paths.files)
        .unwrap_or_default()
        .lines()
        .map(|l| {
            serde_json::from_str::<serde_json::Value>(l).unwrap()["path"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect();
    assert_eq!(touched, ["target.txt"]);
}

/// A project skill lives inside the workspace (`.rupu/skills/`); its files
/// are still not target files, so a workspace read or glob that reaches
/// them writes no touch for them.
#[tokio::test]
async fn a_read_root_inside_the_workspace_is_not_coverage() {
    let ws = tempfile::tempdir().unwrap();
    let skill = ws.path().join(".rupu/skills/pdf");
    fs::create_dir_all(skill.join("references")).unwrap();
    fs::write(skill.join("references/REF.md"), "ref\n").unwrap();
    fs::write(ws.path().join("target.md"), "target\n").unwrap();
    let mut ctx = ToolContext::in_workspace(ws.path());
    ctx.workspace.read_roots = vec![skill.canonicalize().unwrap()];
    let ledger = tempfile::tempdir().unwrap();
    let paths = CoveragePaths::new(ledger.path(), "test-target");
    let handle = CoverageWriterHandle::spawn(paths.clone()).unwrap();
    {
        ctx.services.coverage_writer = Some(handle.writer.clone());
        let calls = [
            (
                &rupu_tools::ReadFileTool as &dyn Tool,
                json!({"path": ".rupu/skills/pdf/references/REF.md"}),
            ),
            (&rupu_tools::GlobTool, json!({"pattern": "**/*.md"})),
        ];
        for (tool, input) in calls {
            let out = tool.invoke(input, &ctx).await.unwrap();
            assert_eq!(out.error, None, "{}", tool.descriptor().name);
            if tool.descriptor().name == "glob" {
                assert_eq!(out.stdout, ".rupu/skills/pdf/references/REF.md\ntarget.md");
            }
        }
        drop(ctx);
    }
    handle.shutdown().await;
    let touched: Vec<(String, String)> = fs::read_to_string(&paths.files)
        .unwrap_or_default()
        .lines()
        .map(|l| {
            let v: serde_json::Value = serde_json::from_str(l).unwrap();
            (
                v["kind"].as_str().unwrap().to_owned(),
                v["path"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    assert_eq!(touched, [("glob".to_owned(), "target.md".to_owned())]);
}

/// Each case is a path the read tools must refuse even though it is spelled
/// from under the read root `<skills>/pdf`.
fn escapes(
    skills: &std::path::Path,
    root: &std::path::Path,
    outside: &std::path::Path,
) -> Vec<std::path::PathBuf> {
    fs::create_dir_all(skills.join("pdf-evil")).unwrap();
    fs::write(skills.join("pdf-evil/f"), "secret\n").unwrap();
    fs::create_dir_all(skills.join("other")).unwrap();
    fs::write(skills.join("other/f"), "secret\n").unwrap();
    fs::write(outside.join("f"), "secret\n").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(outside.join("f"), root.join("link")).unwrap();
    let mut paths = vec![skills.join("pdf-evil/f"), root.join("../other/f")];
    if cfg!(unix) {
        paths.push(root.join("link"));
    }
    paths
}

#[tokio::test]
async fn read_file_refuses_what_only_looks_like_a_read_root() {
    let (_ws, skills, root, ctx) = setup();
    let outside = tempfile::tempdir().unwrap();
    for path in escapes(skills.path(), &root, outside.path()) {
        let out = rupu_tools::ReadFileTool
            .invoke(json!({ "path": path }), &ctx)
            .await
            .unwrap();
        assert!(
            out.error
                .as_deref()
                .unwrap_or_default()
                .contains("escapes workspace"),
            "{}: {:?}",
            path.display(),
            out.error
        );
        assert!(!out.stdout.contains("secret"), "{}", path.display());
    }
}

#[tokio::test]
async fn glob_refuses_what_only_looks_like_a_read_root() {
    let (_ws, skills, root, ctx) = setup();
    let outside = tempfile::tempdir().unwrap();
    for path in escapes(skills.path(), &root, outside.path()) {
        let pattern = path.with_file_name("*");
        let out = rupu_tools::GlobTool
            .invoke(json!({ "pattern": pattern }), &ctx)
            .await
            .unwrap();
        assert_eq!(out.stdout, "", "{}", pattern.display());
    }
}

#[tokio::test]
async fn ast_grep_under_a_read_root_writes_no_coverage_touch() {
    if which::which("ast-grep").is_err() {
        eprintln!("skipping: ast-grep not installed");
        return;
    }
    let (ws, _s, root, mut ctx) = setup();
    fs::write(root.join("helper.rs"), "fn helper() {}\n").unwrap();
    let in_ws = ws.path().join("vendor/skills/lint");
    fs::create_dir_all(&in_ws).unwrap();
    fs::write(in_ws.join("helper.rs"), "fn helper() {}\n").unwrap();
    ctx.workspace.read_roots.push(in_ws.canonicalize().unwrap());
    fs::write(ws.path().join("target.rs"), "fn target() {}\n").unwrap();
    let ledger = tempfile::tempdir().unwrap();
    let paths = CoveragePaths::new(ledger.path(), "test-target");
    let handle = CoverageWriterHandle::spawn(paths.clone()).unwrap();
    {
        ctx.services.coverage_writer = Some(handle.writer.clone());
        for path in [json!(root), json!(".")] {
            let out = rupu_tools::AstGrepTool
                .invoke(
                    json!({ "pattern": "fn $N() {}", "lang": "rust", "path": path }),
                    &ctx,
                )
                .await
                .unwrap();
            assert_eq!(out.error, None);
            assert!(out.stdout.contains("helper.rs:1:1:"), "{}", out.stdout);
        }
        drop(ctx);
    }
    handle.shutdown().await;
    let touched: Vec<String> = fs::read_to_string(&paths.files)
        .unwrap_or_default()
        .lines()
        .map(|l| {
            serde_json::from_str::<serde_json::Value>(l).unwrap()["path"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect();
    assert_eq!(touched, ["target.rs"]);
}
