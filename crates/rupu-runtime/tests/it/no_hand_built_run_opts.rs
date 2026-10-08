//! Spec 2026-10-07 W3 §6 test 2 — the guard that keeps P5 true: no
//! production code outside the run assembler (`rupu-runtime/src/assembly`)
//! builds an `AgentRunOpts` or a `ToolContext` by hand. A launch site states
//! what it wants in a `LaunchSpec`; everything else is derived once.
//!
//! Source-text scan of `crates/*/src/**/*.rs` with `#[cfg(test)]` items
//! blanked out. Declarations (`struct`, `impl`) and destructuring patterns
//! (`let X { .. } =`) are not constructions.
//!
//! TEMPORARY allowances (W3a), each deleted by W3b when it migrates the site
//! onto the assembler: workflow steps (`DefaultStepFactory`) and the
//! agentiflow lead still build the flat `LegacyRunOpts` (and its
//! `ToolContext`), and `rupu-agent`'s `legacy` adapter turns those into
//! `AgentRunOpts`. Nothing else may build either.

use std::path::{Path, PathBuf};

/// `(file, literal)` pairs W3a still allows. W3b empties this list.
const W3B_ADAPTER: &[(&str, &str)] = &[
    (
        "crates/rupu-orchestrator/src/step_factory.rs",
        "LegacyRunOpts",
    ),
    (
        "crates/rupu-orchestrator/src/step_factory.rs",
        "ToolContext",
    ),
    ("crates/rupu-agentiflow/src/lead.rs", "LegacyRunOpts"),
    ("crates/rupu-agentiflow/src/lead.rs", "ToolContext"),
    ("crates/rupu-agent/src/legacy.rs", "AgentRunOpts"),
];

const TYPES: &[&str] = &["AgentRunOpts", "LegacyRunOpts", "ToolContext"];

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root")
        .to_path_buf()
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            rust_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

/// `src` with every `#[cfg(test)]` item (a `mod … { … }`, `fn`, `impl`, …)
/// replaced by spaces, so line structure survives.
fn without_test_items(src: &str) -> String {
    let mut bytes = src.as_bytes().to_vec();
    let mut from = 0;
    while let Some(i) = src[from..].find("#[cfg(test)]") {
        let start = from + i;
        let rest = &src[start..];
        let brace = rest.find('{');
        let semi = rest.find(';');
        let end = match (brace, semi) {
            // `#[cfg(test)] use …;` / `mod x;`
            (Some(b), Some(s)) if s < b => start + s + 1,
            (None, Some(s)) => start + s + 1,
            (Some(b), _) => {
                let mut depth = 0usize;
                let mut end = src.len();
                for (off, c) in src[start + b..].char_indices() {
                    match c {
                        '{' => depth += 1,
                        '}' => {
                            depth -= 1;
                            if depth == 0 {
                                end = start + b + off + 1;
                                break;
                            }
                        }
                        _ => {}
                    }
                }
                end
            }
            (None, None) => src.len(),
        };
        for b in &mut bytes[start..end] {
            if *b != b'\n' {
                *b = b' ';
            }
        }
        from = end;
    }
    String::from_utf8(bytes).expect("blanking keeps UTF-8 boundaries")
}

/// Whether the `{` after `ty` at `at` opens a construction (not a
/// declaration or a destructuring pattern).
fn is_construction(src: &str, at: usize) -> bool {
    let line_start = src[..at].rfind('\n').map_or(0, |i| i + 1);
    let before = src[line_start..at].trim_end();
    let decl = ["struct", "impl", "for", "enum", "let", "trait"]
        .iter()
        .any(|kw| before.ends_with(kw));
    // `X {` as a pattern in a `match` arm or a `let … else` is a
    // destructuring too, but nothing here matches on these types.
    !decl
}

fn hits(src: &str) -> Vec<(String, usize)> {
    let mut out = Vec::new();
    for ty in TYPES {
        let needle = format!("{ty} {{");
        let mut from = 0;
        while let Some(i) = src[from..].find(&needle) {
            let at = from + i;
            let prev = src[..at].chars().next_back();
            let ident_boundary = prev.is_none_or(|c| !(c.is_alphanumeric() || c == '_'));
            if ident_boundary && is_construction(src, at) {
                out.push((ty.to_string(), src[..at].matches('\n').count() + 1));
            }
            from = at + needle.len();
        }
    }
    out
}

#[test]
fn no_hand_built_run_opts() {
    let root = workspace_root();
    let mut files = Vec::new();
    for krate in std::fs::read_dir(root.join("crates")).unwrap().flatten() {
        rust_files(&krate.path().join("src"), &mut files);
    }
    let assembly = root.join("crates/rupu-runtime/src/assembly");
    let mut offenders = Vec::new();
    for file in files {
        if file.starts_with(&assembly) {
            continue;
        }
        let rel = file
            .strip_prefix(&root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        let src = without_test_items(&std::fs::read_to_string(&file).unwrap());
        for (ty, line) in hits(&src) {
            if W3B_ADAPTER.iter().any(|(f, t)| *f == rel && *t == ty) {
                continue;
            }
            offenders.push(format!("{rel}:{line}: `{ty} {{` built by hand"));
        }
    }
    assert!(
        offenders.is_empty(),
        "build runs through `rupu_runtime::assembly::RunAssembler` (a `LaunchSpec`), \
         never by hand:\n{}",
        offenders.join("\n")
    );
}

/// The other half of "one entry point" (§3.1): only the assembler starts the
/// agent loop. Production code outside `rupu-agent` and the assembler never
/// calls `rupu_agent::run_agent*` — except the two W3a adapter sites, which
/// go through `rupu_agent::legacy`.
#[test]
fn only_the_assembler_runs_agents() {
    const ADAPTER_CALLERS: &[&str] = &[
        "crates/rupu-orchestrator/src/runner.rs",
        "crates/rupu-agentiflow/src/lead.rs",
    ];
    let root = workspace_root();
    let mut files = Vec::new();
    for krate in std::fs::read_dir(root.join("crates")).unwrap().flatten() {
        rust_files(&krate.path().join("src"), &mut files);
    }
    let mut offenders = Vec::new();
    for file in files {
        let rel = file
            .strip_prefix(&root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        if rel.starts_with("crates/rupu-agent/")
            || rel.starts_with("crates/rupu-runtime/src/assembly/")
        {
            continue;
        }
        let src = without_test_items(&std::fs::read_to_string(&file).unwrap());
        for (n, line) in src.lines().enumerate() {
            let code = line.split("//").next().unwrap_or("");
            let direct = ["rupu_agent::run_agent", "runner::run_agent"]
                .iter()
                .any(|c| code.contains(c));
            let legacy = code.contains("rupu_agent::legacy::run_agent");
            if direct || (legacy && !ADAPTER_CALLERS.contains(&rel.as_str())) {
                offenders.push(format!("{rel}:{}: {}", n + 1, line.trim()));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "start runs with `rupu_runtime::run_agent` / `AssembledRun::run`:\n{}",
        offenders.join("\n")
    );
}

/// The scanner itself: constructions are found, declarations, patterns and
/// test items are not.
#[test]
fn the_scanner_sees_constructions_only() {
    let src = "pub struct ToolContext {\n}\nimpl ToolContext {\n}\nfn f() {\n    let x = ToolContext {\n    };\n    let AgentRunOpts { a, .. } = o;\n}\n#[cfg(test)]\nmod tests {\n    fn g() { let y = AgentRunOpts { }; }\n}\n";
    let found = hits(&without_test_items(src));
    assert_eq!(found, vec![("ToolContext".to_string(), 6)]);
}
