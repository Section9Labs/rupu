//! `rupu init --with-samples` writes every entry in MANIFEST and the
//! content matches the embedded source byte-for-byte.

use std::path::Path;

use jsonschema::JSONSchema;
use rupu_cli::cmd::init::{init_for_test, InitArgs};
use rupu_cli::templates::MANIFEST;
use rupu_orchestrator::Workflow;

fn args(path: &Path) -> InitArgs {
    InitArgs {
        path: path.to_path_buf(),
        with_samples: true,
        force: false,
        git: false,
    }
}

#[test]
fn with_samples_seeds_every_manifest_entry() {
    let tmp = tempfile::tempdir().unwrap();
    init_for_test(args(tmp.path())).unwrap();

    for t in MANIFEST {
        let p = tmp.path().join(t.target_relpath);
        assert!(p.exists(), "missing template file: {}", t.target_relpath);
        let body = std::fs::read_to_string(&p).unwrap();
        assert_eq!(body, t.content, "content mismatch for {}", t.target_relpath);
    }
}

#[test]
fn samples_byte_match_dogfooded_files() {
    // Catches drift between crates/rupu-cli/templates/* and the
    // .rupu/* files in the rupu repo. If this fails, copy the
    // newer one over the older.
    for t in MANIFEST {
        let workspace_relpath = format!("../../{}", t.target_relpath);
        let on_disk_path =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(&workspace_relpath);
        let on_disk = std::fs::read_to_string(&on_disk_path).unwrap_or_else(|e| {
            panic!(
                "could not read dogfooded source {}: {e}",
                on_disk_path.display()
            )
        });
        assert_eq!(
            on_disk, t.content,
            "drift between {} (rupu repo) and the embedded template",
            t.target_relpath
        );
    }
}

#[test]
fn sample_contract_schemas_compile() {
    for t in MANIFEST
        .iter()
        .filter(|t| t.target_relpath.starts_with(".rupu/contracts/"))
    {
        let schema_json: serde_json::Value = serde_json::from_str(t.content)
            .unwrap_or_else(|e| panic!("contract {} is not valid JSON: {e}", t.target_relpath));
        JSONSchema::compile(&schema_json)
            .unwrap_or_else(|e| panic!("contract {} does not compile: {e}", t.target_relpath));
    }
}

#[test]
fn sample_workflows_parse() {
    for t in MANIFEST
        .iter()
        .filter(|t| t.target_relpath.starts_with(".rupu/workflows/"))
    {
        Workflow::parse(t.content)
            .unwrap_or_else(|e| panic!("workflow {} failed to parse: {e}", t.target_relpath));
    }
}
