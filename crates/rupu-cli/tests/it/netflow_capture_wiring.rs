//! Text-level BACKSTOP: every production `ToolContext { .. }` literal on a
//! run path must set `netflow_sink:` and `net_capture:` to something other
//! than `None`, or `bash` silently loses subprocess network capture for that
//! entry point (a `ToolContext` that "compiles fine" with both left `None`).
//!
//! Like `rupu-netflow`'s `choke_point` test this is secondary and
//! structurally blunt: it matches source text, not types, so it cannot see a
//! `ToolContext` built via a helper or `..base` spread. The type system still
//! forces the fields to be spelled out (no `Default` on `ToolContext` is used
//! at these sites), which is what makes the text check meaningful. Since W3
//! the run assembler builds every run's `ToolContext` from its
//! `AssemblyContext`, so it also checks that every production
//! `AssemblyContext` carries the process-wide capture (the assembler hands
//! it to each run it builds, dispatched children included).

use std::path::{Path, PathBuf};

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root")
        .to_path_buf()
}

/// Source with its trailing `#[cfg(test)]` module removed.
fn production_source(rel: &str) -> String {
    let src = std::fs::read_to_string(workspace_root().join(rel))
        .unwrap_or_else(|e| panic!("read {rel}: {e}"));
    match src.find("#[cfg(test)]") {
        Some(i) => src[..i].to_string(),
        None => src,
    }
}

/// The text of every `ToolContext { ... }` literal (brace-balanced).
fn tool_context_blocks(src: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(i) = src[from..].find("ToolContext {") {
        let start = from + i;
        let open = start + "ToolContext ".len();
        let mut depth = 0usize;
        let mut end = open;
        for (off, c) in src[open..].char_indices() {
            match c {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        end = open + off + 1;
                        break;
                    }
                }
                _ => {}
            }
        }
        out.push(src[start..end].to_string());
        from = end;
    }
    out
}

fn field_is_set(block: &str, field: &str) -> bool {
    block.lines().any(|l| {
        let l = l.trim();
        l.starts_with(&format!("{field}:")) && !l.starts_with(&format!("{field}: None"))
    })
}

#[test]
fn every_production_tool_context_sets_sink_and_capture() {
    for rel in [
        "crates/rupu-runtime/src/assembly/mod.rs",
        // The flat options workflow steps still build (W3b moves them onto
        // the assembler).
        "crates/rupu-orchestrator/src/step_factory.rs",
    ] {
        let blocks = tool_context_blocks(&production_source(rel));
        assert!(!blocks.is_empty(), "{rel}: no production ToolContext found");
        for b in blocks {
            assert!(
                field_is_set(&b, "netflow_sink"),
                "{rel}: a production ToolContext leaves netflow_sink None:\n{b}"
            );
            assert!(
                field_is_set(&b, "net_capture"),
                "{rel}: a production ToolContext leaves net_capture None:\n{b}"
            );
        }
    }
}

#[test]
fn every_production_assembler_gets_the_capture() {
    for rel in [
        "crates/rupu-cli/src/cmd/run.rs",
        "crates/rupu-cli/src/cmd/session.rs",
        "crates/rupu-cli/src/cmd/workflow.rs",
        "crates/rupu-cli/src/resume.rs",
    ] {
        let src = production_source(rel);
        let mut seen = 0;
        for chunk in src.split("AssemblyContext {").skip(1) {
            seen += 1;
            let body = &chunk[..chunk.find("},").expect("literal end")];
            assert!(
                body.contains("net_capture: Some("),
                "{rel}: AssemblyContext without net_capture: Some(..)"
            );
        }
        assert!(seen > 0, "{rel}: expected a production assembler");
    }
}

#[test]
fn every_production_step_factory_gets_a_capture() {
    for rel in [
        "crates/rupu-cli/src/cmd/workflow.rs",
        "crates/rupu-cli/src/resume.rs",
    ] {
        let src = production_source(rel);
        for chunk in src.split("DefaultStepFactory {").skip(1) {
            let body = &chunk[..chunk.find("});").expect("literal end")];
            assert!(
                body.contains("net_capture: Some("),
                "{rel}: DefaultStepFactory without net_capture: Some(..)"
            );
        }
    }
}
