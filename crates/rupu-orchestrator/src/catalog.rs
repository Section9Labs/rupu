//! Library-level workflow lister.
//!
//! `rupu-cli` and `rupu-cp` each carry a private copy of "scan the
//! workflows directories"; this is the shared, parse-aware version a
//! caller that wants to describe the available workflows (e.g. the
//! agentiflow lead's roster) can use. It is deliberately tolerant: a
//! file that fails to parse is still listed, carrying its parse error,
//! so one bad file never hides the rest of the catalog.

use crate::workflow::Workflow;
use std::collections::BTreeMap;
use std::path::Path;

/// One workflow as the catalog sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowSummary {
    /// The runnable identifier: the file stem (`<id>.yaml`). Workflows
    /// are resolved and shadowed by stem everywhere else in rupu, and
    /// the parser does not require `name:` to match it.
    pub id: String,
    /// The workflow's declared `name:` (for display); the file stem
    /// when the file did not parse.
    pub name: String,
    pub description: Option<String>,
    /// `"global"` or `"project"`.
    pub scope: String,
    /// Declared input names, sorted.
    pub input_keys: Vec<String>,
    pub step_count: usize,
    /// `Some` when the file did not parse (the entry is still listed,
    /// with the other fields empty).
    pub parse_error: Option<String>,
}

/// List the available workflows.
///
/// Scans `<global>/workflows/*.yaml` (scope `"global"`) and, when
/// `project` is `Some`, `<project>/workflows/*.yaml` (scope
/// `"project"`). `global` is the global rupu root (`~/.rupu`);
/// `project` is the project's `.rupu` directory, matching
/// `rupu_agent::load_agents`, so one `(global, project)` pair serves
/// both. A project workflow shadows a global one with the same file
/// stem (`id`), even when the project file does not parse. A missing
/// directory yields no entries. A file that fails to parse is listed
/// with `parse_error: Some(..)` and the scan continues. The result is
/// sorted by `id`.
pub fn list_workflow_summaries(global: &Path, project: Option<&Path>) -> Vec<WorkflowSummary> {
    let mut by_name: BTreeMap<String, WorkflowSummary> = BTreeMap::new();
    scan_dir(&global.join("workflows"), "global", &mut by_name);
    if let Some(p) = project {
        scan_dir(&p.join("workflows"), "project", &mut by_name);
    }
    by_name.into_values().collect()
}

/// Load one workflow by its runnable id (the file stem).
///
/// Resolves exactly as [`list_workflow_summaries`] does: the project's
/// `<project>/workflows/<id>.yaml` (when `project` is `Some`; `project`
/// is the `.rupu` directory) shadows `<global>/workflows/<id>.yaml`,
/// even when the project file does not parse. Returns `None` when the
/// workflow is not found, does not parse, or `id` is not a plain file
/// stem (empty, or containing a path separator or `..`). Only `.yaml`
/// is considered, matching the lister.
pub fn load_workflow(global: &Path, project: Option<&Path>, id: &str) -> Option<Workflow> {
    if id.is_empty() || id.contains(['/', '\\']) || id.contains("..") {
        return None;
    }
    let file = format!("{id}.yaml");
    let path = project
        .map(|p| p.join("workflows").join(&file))
        .filter(|p| p.is_file())
        .or_else(|| Some(global.join("workflows").join(&file)).filter(|p| p.is_file()))?;
    Workflow::parse_file(&path).ok()
}

fn scan_dir(dir: &Path, scope: &str, into: &mut BTreeMap<String, WorkflowSummary>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    // `read_dir` order is platform-defined; sort for deterministic iteration.
    // Entries are keyed and shadowed by file stem below, and stems are unique
    // within a scope, so this order doesn't affect which entry wins; the final
    // result is id-sorted regardless.
    let mut paths: Vec<_> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.extension().and_then(|s| s.to_str()) == Some("yaml"))
        .collect();
    paths.sort();
    for path in paths {
        let id = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_string();
        let summary = match Workflow::parse_file(&path) {
            Ok(wf) => WorkflowSummary {
                id: id.clone(),
                name: wf.name,
                description: wf.description,
                scope: scope.to_string(),
                input_keys: wf.inputs.keys().cloned().collect(),
                step_count: wf.steps.len(),
                parse_error: None,
            },
            Err(e) => WorkflowSummary {
                id: id.clone(),
                name: id.clone(),
                description: None,
                scope: scope.to_string(),
                input_keys: Vec::new(),
                step_count: 0,
                parse_error: Some(e.to_string()),
            },
        };
        into.insert(id, summary);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TWO_STEPS: &str = "\
name: two-step
description: A two step workflow.
inputs:
  topic:
    type: string
steps:
  - id: first
    agent: writer
    prompt: write about {{ inputs.topic }}
  - id: second
    agent: reviewer
    prompt: review {{ steps.first.output }}
";

    fn write(dir: &Path, rel: &str, body: &str) {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }

    const ONE_STEP: &str = "name: foo\nsteps:\n  - id: only\n    agent: writer\n    prompt: hi\n";

    #[test]
    fn lists_valid_and_broken_files_without_aborting() {
        let tmp = tempfile::tempdir().unwrap();
        let global = tmp.path().join("global");
        write(&global, "workflows/two-step.yaml", TWO_STEPS);
        write(
            &global,
            "workflows/broken.yaml",
            "name: [unclosed\nsteps: {",
        );
        write(&global, "workflows/ignored.yml", TWO_STEPS);
        write(&global, "workflows/notes.txt", "not a workflow");

        let got = list_workflow_summaries(&global, None);
        assert_eq!(
            got.len(),
            2,
            "both .yaml files listed, others ignored: {got:?}"
        );

        let broken = got
            .iter()
            .find(|s| s.id == "broken")
            .expect("broken listed");
        assert_eq!(broken.name, "broken");
        assert!(broken.parse_error.is_some());
        assert_eq!(broken.step_count, 0);
        assert_eq!(broken.scope, "global");

        let ok = got
            .iter()
            .find(|s| s.id == "two-step")
            .expect("valid listed");
        assert_eq!(ok.name, "two-step");
        assert_eq!(ok.parse_error, None);
        assert_eq!(ok.step_count, 2);
        assert_eq!(ok.scope, "global");
        assert_eq!(ok.description.as_deref(), Some("A two step workflow."));
        assert_eq!(ok.input_keys, vec!["topic".to_string()]);
    }

    #[test]
    fn project_workflow_shadows_global_by_file_stem() {
        let tmp = tempfile::tempdir().unwrap();
        let global = tmp.path().join("global");
        let project = tmp.path().join("proj").join(".rupu");
        write(
            &global,
            "workflows/foo.yaml",
            TWO_STEPS.replace("two-step", "foo").as_str(),
        );
        write(&project, "workflows/foo.yaml", ONE_STEP);

        let got = list_workflow_summaries(&global, Some(&project));
        assert_eq!(got.len(), 1, "shadowed, not duplicated: {got:?}");
        assert_eq!(got[0].id, "foo");
        assert_eq!(got[0].name, "foo");
        assert_eq!(got[0].scope, "project");
        assert_eq!(got[0].step_count, 1);
    }

    #[test]
    fn keyed_by_file_stem_not_declared_name() {
        let tmp = tempfile::tempdir().unwrap();
        let global = tmp.path().join("global");
        let project = tmp.path().join("proj").join(".rupu");
        // `foo.yaml` declares `name: bar`: it runs as `foo`.
        write(
            &global,
            "workflows/foo.yaml",
            &ONE_STEP.replace("name: foo", "name: bar"),
        );
        // A project `bar.yaml` declaring `name: foo` must NOT shadow it.
        write(&project, "workflows/bar.yaml", ONE_STEP);

        let got = list_workflow_summaries(&global, Some(&project));
        assert_eq!(got.len(), 2, "distinct stems stay distinct: {got:?}");
        let foo = got.iter().find(|s| s.id == "foo").unwrap();
        assert_eq!((foo.name.as_str(), foo.scope.as_str()), ("bar", "global"));
        let bar = got.iter().find(|s| s.id == "bar").unwrap();
        assert_eq!((bar.name.as_str(), bar.scope.as_str()), ("foo", "project"));
    }

    #[test]
    fn broken_project_file_still_shadows_valid_global_by_stem() {
        let tmp = tempfile::tempdir().unwrap();
        let global = tmp.path().join("global");
        let project = tmp.path().join("proj").join(".rupu");
        write(&global, "workflows/foo.yaml", ONE_STEP);
        write(&project, "workflows/foo.yaml", "name: [unclosed\nsteps: {");

        let got = list_workflow_summaries(&global, Some(&project));
        assert_eq!(got.len(), 1, "one foo entry: {got:?}");
        assert_eq!(got[0].id, "foo");
        assert_eq!(got[0].scope, "project");
        assert!(got[0].parse_error.is_some());
    }

    #[test]
    fn missing_directories_yield_no_entries() {
        let tmp = tempfile::tempdir().unwrap();
        let got = list_workflow_summaries(&tmp.path().join("nope"), Some(&tmp.path().join("nada")));
        assert!(got.is_empty());
    }

    #[test]
    fn load_workflow_finds_global_by_file_stem() {
        let tmp = tempfile::tempdir().unwrap();
        let global = tmp.path().join("global");
        // `foo.yaml` declares `name: bar`: it loads as `foo`.
        write(
            &global,
            "workflows/foo.yaml",
            &ONE_STEP.replace("name: foo", "name: bar"),
        );
        let wf = load_workflow(&global, None, "foo").expect("found by stem");
        assert_eq!(wf.name, "bar");
        assert_eq!(wf.steps.len(), 1);
        assert!(load_workflow(&global, None, "bar").is_none(), "not by name");
    }

    #[test]
    fn load_workflow_project_shadows_global() {
        let tmp = tempfile::tempdir().unwrap();
        let global = tmp.path().join("global");
        let project = tmp.path().join("proj").join(".rupu");
        write(&global, "workflows/foo.yaml", TWO_STEPS);
        write(&project, "workflows/foo.yaml", ONE_STEP);
        let wf = load_workflow(&global, Some(&project), "foo").expect("found");
        assert_eq!(wf.steps.len(), 1, "project copy wins");
        // Falls back to global when the project has no such file.
        write(&global, "workflows/only-global.yaml", TWO_STEPS);
        let wf = load_workflow(&global, Some(&project), "only-global").expect("global fallback");
        assert_eq!(wf.steps.len(), 2);
    }

    #[test]
    fn load_workflow_none_for_missing_garbage_and_bad_ids() {
        let tmp = tempfile::tempdir().unwrap();
        let global = tmp.path().join("global");
        write(
            &global,
            "workflows/broken.yaml",
            "name: [unclosed\nsteps: {",
        );
        write(&global, "workflows/foo.yaml", ONE_STEP);
        assert!(load_workflow(&global, None, "missing").is_none());
        assert!(load_workflow(&global, None, "broken").is_none());
        assert!(load_workflow(&global, None, "").is_none());
        assert!(load_workflow(&global, None, "../workflows/foo").is_none());
        assert!(load_workflow(&global, None, "sub/foo").is_none());
        // A broken project file shadows a good global one (as in the lister).
        let project = tmp.path().join("proj").join(".rupu");
        write(&project, "workflows/foo.yaml", "name: [unclosed\nsteps: {");
        assert!(load_workflow(&global, Some(&project), "foo").is_none());
    }
}
