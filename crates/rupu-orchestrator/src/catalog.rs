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
    /// The workflow's declared `name:`; the file stem when the file
    /// did not parse.
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
/// `project` is `Some`, `<project>/.rupu/workflows/*.yaml` (scope
/// `"project"`). `global` is the global rupu root (`~/.rupu`);
/// `project` is the directory CONTAINING `.rupu` (the project root).
/// A project workflow shadows a global one of the same name. A missing
/// directory yields no entries. A file that fails to parse is listed
/// with `parse_error: Some(..)` and the scan continues. The result is
/// sorted by name.
pub fn list_workflow_summaries(global: &Path, project: Option<&Path>) -> Vec<WorkflowSummary> {
    let mut by_name: BTreeMap<String, WorkflowSummary> = BTreeMap::new();
    scan_dir(&global.join("workflows"), "global", &mut by_name);
    if let Some(p) = project {
        scan_dir(&p.join(".rupu").join("workflows"), "project", &mut by_name);
    }
    by_name.into_values().collect()
}

fn scan_dir(dir: &Path, scope: &str, into: &mut BTreeMap<String, WorkflowSummary>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    // `read_dir` order is platform-defined; sort so two files that
    // declare the same `name:` in one scope resolve deterministically.
    let mut paths: Vec<_> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.extension().and_then(|s| s.to_str()) == Some("yaml"))
        .collect();
    paths.sort();
    for path in paths {
        let summary = match Workflow::parse_file(&path) {
            Ok(wf) => WorkflowSummary {
                name: wf.name,
                description: wf.description,
                scope: scope.to_string(),
                input_keys: wf.inputs.keys().cloned().collect(),
                step_count: wf.steps.len(),
                parse_error: None,
            },
            Err(e) => WorkflowSummary {
                name: path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or_default()
                    .to_string(),
                description: None,
                scope: scope.to_string(),
                input_keys: Vec::new(),
                step_count: 0,
                parse_error: Some(e.to_string()),
            },
        };
        into.insert(summary.name.clone(), summary);
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
            .find(|s| s.name == "broken")
            .expect("broken listed");
        assert!(broken.parse_error.is_some());
        assert_eq!(broken.step_count, 0);
        assert_eq!(broken.scope, "global");

        let ok = got
            .iter()
            .find(|s| s.name == "two-step")
            .expect("valid listed");
        assert_eq!(ok.parse_error, None);
        assert_eq!(ok.step_count, 2);
        assert_eq!(ok.scope, "global");
        assert_eq!(ok.description.as_deref(), Some("A two step workflow."));
        assert_eq!(ok.input_keys, vec!["topic".to_string()]);
    }

    #[test]
    fn project_workflow_shadows_global_by_name() {
        let tmp = tempfile::tempdir().unwrap();
        let global = tmp.path().join("global");
        let project = tmp.path().join("proj");
        write(
            &global,
            "workflows/foo.yaml",
            TWO_STEPS.replace("two-step", "foo").as_str(),
        );
        write(
            &project,
            ".rupu/workflows/foo.yaml",
            "name: foo\nsteps:\n  - id: only\n    agent: writer\n    prompt: hi\n",
        );

        let got = list_workflow_summaries(&global, Some(&project));
        assert_eq!(got.len(), 1, "shadowed, not duplicated: {got:?}");
        assert_eq!(got[0].name, "foo");
        assert_eq!(got[0].scope, "project");
        assert_eq!(got[0].step_count, 1);
    }

    #[test]
    fn missing_directories_yield_no_entries() {
        let tmp = tempfile::tempdir().unwrap();
        let got = list_workflow_summaries(&tmp.path().join("nope"), Some(&tmp.path().join("nada")));
        assert!(got.is_empty());
    }
}
