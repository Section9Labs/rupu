//! `rupu coverage` — inspect agentic coverage ledgers and concern catalogs.

use crate::output::formats::OutputFormat;
use anyhow::Result;
use clap::Subcommand;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

#[derive(Subcommand, Debug)]
pub enum Action {
    /// List coverage targets recorded under .rupu/coverage/.
    List,
    /// List or show bundled concern templates.
    Templates {
        #[command(subcommand)]
        action: TemplatesAction,
    },
    /// Print the effective catalog snapshot for a target.
    Catalog {
        /// Target id (from `coverage list`).
        target_id: String,
    },
    /// Show the derived ledger view (touched files + assertions + findings).
    Show {
        /// Target id (from `coverage list`).
        target_id: String,
    },
    /// Generate the coverage audit report for a target.
    Audit {
        /// Target id (from `coverage list`).
        target_id: String,
        /// Emit machine-readable JSON instead of the human summary.
        #[arg(long)]
        json: bool,
    },
    /// Show only the coverage gaps (in-scope files lacking an assertion).
    Gap {
        /// Target id (from `coverage list`).
        target_id: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum TemplatesAction {
    /// List bundled template names.
    List,
    /// Print a bundled template's concerns.
    Show { name: String },
}

fn workspace() -> Result<PathBuf> {
    Ok(std::env::current_dir()?)
}

pub async fn handle(action: Action, _format: Option<OutputFormat>) -> ExitCode {
    let result = match action {
        Action::List => workspace().and_then(|ws| run_list_in(&ws)),
        Action::Templates { action } => run_templates(action),
        Action::Catalog { target_id } => workspace().and_then(|ws| run_catalog_in(&ws, &target_id)),
        Action::Show { target_id } => workspace().and_then(|ws| run_show_in(&ws, &target_id)),
        Action::Audit { target_id, json } => {
            workspace().and_then(|ws| run_audit_in(&ws, &target_id, json))
        }
        Action::Gap { target_id } => workspace().and_then(|ws| run_gap_in(&ws, &target_id)),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("coverage error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run_list_in(workspace: &Path) -> Result<()> {
    let targets = rupu_coverage::discover_targets(workspace)?;
    if targets.is_empty() {
        println!("no coverage targets under .rupu/coverage/");
        return Ok(());
    }
    for t in targets {
        println!(
            "{}  ·  {} assertions  ·  catalog: {}",
            t.target_id,
            t.assertion_lines,
            if t.has_catalog { "yes" } else { "no" }
        );
    }
    Ok(())
}

fn run_templates(action: TemplatesAction) -> Result<()> {
    match action {
        TemplatesAction::List => {
            for name in rupu_coverage::builtin_names() {
                println!("{name}");
            }
            Ok(())
        }
        TemplatesAction::Show { name } => {
            let template = rupu_coverage::resolve_builtin(&name)
                .ok_or_else(|| anyhow::anyhow!("unknown template `{name}`"))?
                .map_err(|e| anyhow::anyhow!("template parse error: {e}"))?;
            for concern in &template.concerns {
                println!("{}  [{:?}]  {}", concern.id, concern.severity, concern.name);
            }
            Ok(())
        }
    }
}

fn run_catalog_in(workspace: &Path, target_id: &str) -> Result<()> {
    let paths = rupu_coverage::CoveragePaths::new(workspace, target_id);
    if !paths.catalog.exists() {
        anyhow::bail!("no catalog snapshot for target `{target_id}`");
    }
    let catalog = rupu_coverage::read_snapshot(&paths.catalog)?;
    println!("{} concerns in effective catalog", catalog.concerns.len());
    for c in &catalog.concerns {
        println!("  {}  [{:?}]  {}", c.id, c.severity, c.name);
    }
    Ok(())
}

fn run_show_in(workspace: &Path, target_id: &str) -> Result<()> {
    let paths = rupu_coverage::CoveragePaths::new(workspace, target_id);
    let events = rupu_coverage::read_file_events(&paths)?;
    let views = rupu_coverage::file_views(&events);
    let assertions = rupu_coverage::read_concern_assertions(&paths)?;
    let findings = rupu_coverage::read_findings(&paths)?;

    println!("== files touched ({}) ==", views.len());
    for v in &views {
        println!("  {}  [{}]", v.path, format!("{:?}", v.strongest).to_lowercase());
    }
    println!("== concern assertions ({}) ==", assertions.len());
    for a in &assertions {
        println!(
            "  {} · {} · {:?} · {}",
            a.concern_id, a.file_path, a.status, a.declared_by.model
        );
    }
    println!("== findings ({}) ==", findings.len());
    for f in &findings {
        println!(
            "  {} · {:?} · {} · {}",
            f.id,
            f.severity,
            f.file_path.as_deref().unwrap_or("(repo)"),
            f.summary
        );
    }
    Ok(())
}

fn run_audit_in(workspace: &Path, target_id: &str, json: bool) -> Result<()> {
    let paths = rupu_coverage::CoveragePaths::new(workspace, target_id);
    let report = rupu_coverage::run_audit(&paths)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }
    println!(
        "coverage audit · target {} · {}/{} concerns complete · {} gap files",
        report.target_id,
        report.complete_concerns,
        report.total_concerns,
        report.total_gap_files
    );
    println!();
    println!("== per-concern ==");
    for c in &report.concerns {
        let mark = if c.is_complete() { "ok" } else { "GAP" };
        println!(
            "  [{}] {}  [{:?}]  in_scope={} asserted={} gap={}  (clean {} / finding {} / examined {} / n/a {})",
            mark,
            c.concern_id,
            c.severity,
            c.in_scope_files.len(),
            c.asserted_files.len(),
            c.gap_files.len(),
            c.clean,
            c.findings,
            c.examined,
            c.not_applicable,
        );
    }
    if !report.cross_model.is_empty() {
        println!();
        println!("== cross-model ==");
        for x in &report.cross_model {
            let tag = if x.disagreement { "DISAGREE" } else { "agree" };
            println!(
                "  [{}] {} · {} · {:?}",
                tag, x.concern_id, x.file_path, x.model_statuses
            );
        }
    }
    if !report.serendipitous.is_empty() {
        println!();
        println!("== serendipitous findings ==");
        for s in &report.serendipitous {
            println!("  ({}) {}  {:?}", s.count, s.theme, s.finding_ids);
        }
    }
    Ok(())
}

fn run_gap_in(workspace: &Path, target_id: &str) -> Result<()> {
    let paths = rupu_coverage::CoveragePaths::new(workspace, target_id);
    let report = rupu_coverage::run_audit(&paths)?;
    let mut any = false;
    for c in &report.concerns {
        if c.gap_files.is_empty() {
            continue;
        }
        any = true;
        println!("{} ({} gap files):", c.concern_id, c.gap_files.len());
        for f in &c.gap_files {
            println!("  {f}");
        }
    }
    if !any {
        println!("no coverage gaps");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_in_handles_no_targets() {
        let tmp = tempfile::TempDir::new().unwrap();
        // No .rupu/coverage → prints the empty message, returns Ok.
        assert!(run_list_in(tmp.path()).is_ok());
    }

    #[test]
    fn templates_list_runs() {
        assert!(run_templates(TemplatesAction::List).is_ok());
    }

    #[test]
    fn templates_show_unknown_errors() {
        assert!(run_templates(TemplatesAction::Show { name: "nope".into() }).is_err());
    }

    #[test]
    fn templates_show_known_runs() {
        assert!(run_templates(TemplatesAction::Show { name: "stride".into() }).is_ok());
    }

    #[test]
    fn catalog_missing_snapshot_errors() {
        let tmp = tempfile::TempDir::new().unwrap();
        assert!(run_catalog_in(tmp.path(), "missing").is_err());
    }

    #[test]
    fn show_empty_target_is_ok() {
        let tmp = tempfile::TempDir::new().unwrap();
        // No ledger files → empty sections, no error.
        assert!(run_show_in(tmp.path(), "missing").is_ok());
    }

    #[test]
    fn catalog_prints_snapshot_concerns() {
        use rupu_coverage::{
            flatten, write_snapshot, CatalogMode, ConcernsBlock, ConcernsEntry, CoveragePaths,
            IncludeDirective,
        };
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = CoveragePaths::new(tmp.path(), "tgt");
        paths.ensure_dir().unwrap();
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
        assert!(run_catalog_in(tmp.path(), "tgt").is_ok());
    }

    #[test]
    fn audit_on_populated_target_json_and_human() {
        use rupu_coverage::{
            flatten, write_snapshot, CoveragePaths, CatalogMode, ConcernsBlock, ConcernsEntry,
            IncludeDirective, AssertionStatus, Attribution, ConcernAssertion, Evidence,
            FileTouchEvent, Surface,
        };
        use chrono::Utc;

        let tmp = tempfile::TempDir::new().unwrap();
        let paths = CoveragePaths::new(tmp.path(), "tgt");
        paths.ensure_dir().unwrap();
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
        std::fs::write(
            &paths.files,
            serde_json::to_string(&touch).unwrap() + "\n",
        )
        .unwrap();
        let a = ConcernAssertion {
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
            serde_json::to_string(&a).unwrap() + "\n",
        )
        .unwrap();

        assert!(run_audit_in(tmp.path(), "tgt", true).is_ok()); // json
        assert!(run_audit_in(tmp.path(), "tgt", false).is_ok()); // human
        assert!(run_gap_in(tmp.path(), "tgt").is_ok());
    }
}
