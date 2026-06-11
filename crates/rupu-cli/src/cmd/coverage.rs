//! `rupu coverage` — inspect agentic coverage ledgers and concern catalogs.

use crate::output::formats::OutputFormat;
use crate::output::report::{self, CollectionOutput, DetailOutput};
use anyhow::Result;
use clap::Subcommand;
use serde::Serialize;
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
    },
    /// Show only the coverage gaps (in-scope files lacking an assertion).
    Gap {
        /// Target id (from `coverage list`).
        target_id: String,
    },
    /// Diff two runs against a target (defaults to `previous latest`).
    Diff {
        /// Target id (from `coverage list`).
        target_id: String,
        /// Base run selector: a run id, `latest`, or `previous`.
        base: Option<String>,
        /// Compare run selector: a run id, `latest`, or `previous`.
        compare: Option<String>,
    },
    /// List the runs recorded against a target (to find ids to diff).
    Runs {
        /// Target id (from `coverage list`).
        target_id: String,
    },
    /// Replay an agent run by id, appending a new run to the same target.
    Rerun {
        /// Target id (from `coverage list`).
        target_id: String,
        /// Run id to replay (from `coverage runs`).
        run_id: String,
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

pub async fn handle(action: Action, format: Option<OutputFormat>) -> ExitCode {
    // `rerun` dispatches a full sub-run (async) and owns its own exit code.
    if let Action::Rerun { target_id, run_id } = action {
        return run_rerun_in(&target_id, &run_id).await;
    }

    let result = match action {
        Action::List => workspace().and_then(|ws| run_list_in(&ws, format)),
        Action::Templates { action } => run_templates(action, format),
        Action::Catalog { target_id } => {
            workspace().and_then(|ws| run_catalog_in(&ws, &target_id, format))
        }
        Action::Show { target_id } => {
            workspace().and_then(|ws| run_show_in(&ws, &target_id, format))
        }
        Action::Audit { target_id } => {
            workspace().and_then(|ws| run_audit_in(&ws, &target_id, format))
        }
        Action::Gap { target_id } => workspace().and_then(|ws| run_gap_in(&ws, &target_id, format)),
        Action::Diff {
            target_id,
            base,
            compare,
        } => workspace().and_then(|ws| run_diff_in(&ws, &target_id, base, compare, format)),
        Action::Runs { target_id } => {
            workspace().and_then(|ws| run_runs_in(&ws, &target_id, format))
        }
        Action::Rerun { .. } => unreachable!("handled above"),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("coverage error: {e}");
            ExitCode::FAILURE
        }
    }
}

pub fn ensure_output_format(action: &Action, format: OutputFormat) -> anyhow::Result<()> {
    let (command_name, supported) = match action {
        Action::List => ("coverage list", report::TABLE_JSON_CSV),
        Action::Templates {
            action: TemplatesAction::List,
        } => ("coverage templates list", report::TABLE_JSON_CSV),
        Action::Templates {
            action: TemplatesAction::Show { .. },
        } => ("coverage templates show", report::TABLE_JSON_CSV),
        Action::Catalog { .. } => ("coverage catalog", report::TABLE_JSON_CSV),
        Action::Show { .. } => ("coverage show", report::TABLE_JSON),
        Action::Audit { .. } => ("coverage audit", report::TABLE_JSON),
        Action::Gap { .. } => ("coverage gap", report::TABLE_JSON_CSV),
        Action::Diff { .. } => ("coverage diff", report::TABLE_JSON),
        Action::Runs { .. } => ("coverage runs", report::TABLE_JSON_CSV),
        Action::Rerun { .. } => ("coverage rerun", report::TABLE_ONLY),
    };
    crate::output::formats::ensure_supported(command_name, format, supported)
}

// ── list ──────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
struct CoverageTargetRow {
    target_id: String,
    assertions: usize,
    has_catalog: bool,
}

#[derive(Debug, Clone, Serialize)]
struct CoverageTargetsReport {
    kind: &'static str,
    version: u8,
    rows: Vec<CoverageTargetRow>,
}

struct CoverageListOutput {
    report: CoverageTargetsReport,
}

impl CollectionOutput for CoverageListOutput {
    type JsonReport = CoverageTargetsReport;
    type CsvRow = CoverageTargetRow;

    fn command_name(&self) -> &'static str {
        "coverage list"
    }

    fn json_report(&self) -> &Self::JsonReport {
        &self.report
    }

    fn csv_rows(&self) -> &[Self::CsvRow] {
        &self.report.rows
    }

    fn csv_headers(&self) -> Option<&'static [&'static str]> {
        Some(&["target_id", "assertions", "has_catalog"])
    }

    fn render_table(&self) -> anyhow::Result<()> {
        if self.report.rows.is_empty() {
            println!("no coverage targets under .rupu/coverage/");
            return Ok(());
        }
        for t in &self.report.rows {
            println!(
                "{}  ·  {} assertions  ·  catalog: {}",
                t.target_id,
                t.assertions,
                if t.has_catalog { "yes" } else { "no" }
            );
        }
        Ok(())
    }
}

fn run_list_in(workspace: &Path, format: Option<OutputFormat>) -> Result<()> {
    let targets = rupu_coverage::discover_targets(workspace)?;
    let rows = targets
        .into_iter()
        .map(|t| CoverageTargetRow {
            target_id: t.target_id,
            assertions: t.assertion_lines,
            has_catalog: t.has_catalog,
        })
        .collect::<Vec<_>>();
    let output = CoverageListOutput {
        report: CoverageTargetsReport {
            kind: "coverage_targets",
            version: 1,
            rows,
        },
    };
    report::emit_collection(format, &output)
}

// ── templates list ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
struct TemplateNameRow {
    name: String,
}

#[derive(Debug, Clone, Serialize)]
struct CoverageTemplatesReport {
    kind: &'static str,
    version: u8,
    rows: Vec<TemplateNameRow>,
}

struct TemplatesListOutput {
    report: CoverageTemplatesReport,
}

impl CollectionOutput for TemplatesListOutput {
    type JsonReport = CoverageTemplatesReport;
    type CsvRow = TemplateNameRow;

    fn command_name(&self) -> &'static str {
        "coverage templates list"
    }

    fn json_report(&self) -> &Self::JsonReport {
        &self.report
    }

    fn csv_rows(&self) -> &[Self::CsvRow] {
        &self.report.rows
    }

    fn csv_headers(&self) -> Option<&'static [&'static str]> {
        Some(&["name"])
    }

    fn render_table(&self) -> anyhow::Result<()> {
        for r in &self.report.rows {
            println!("{}", r.name);
        }
        Ok(())
    }
}

// ── templates show ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
struct ConcernRow {
    id: String,
    severity: String,
    name: String,
}

#[derive(Debug, Clone, Serialize)]
struct CoverageTemplateConcernsReport {
    kind: &'static str,
    version: u8,
    rows: Vec<ConcernRow>,
}

struct TemplatesShowOutput {
    report: CoverageTemplateConcernsReport,
}

impl CollectionOutput for TemplatesShowOutput {
    type JsonReport = CoverageTemplateConcernsReport;
    type CsvRow = ConcernRow;

    fn command_name(&self) -> &'static str {
        "coverage templates show"
    }

    fn json_report(&self) -> &Self::JsonReport {
        &self.report
    }

    fn csv_rows(&self) -> &[Self::CsvRow] {
        &self.report.rows
    }

    fn csv_headers(&self) -> Option<&'static [&'static str]> {
        Some(&["id", "severity", "name"])
    }

    fn render_table(&self) -> anyhow::Result<()> {
        for c in &self.report.rows {
            println!("{}  [{}]  {}", c.id, c.severity, c.name);
        }
        Ok(())
    }
}

fn run_templates(action: TemplatesAction, format: Option<OutputFormat>) -> Result<()> {
    match action {
        TemplatesAction::List => {
            let rows = rupu_coverage::builtin_names()
                .map(|name| TemplateNameRow {
                    name: name.to_string(),
                })
                .collect::<Vec<_>>();
            let output = TemplatesListOutput {
                report: CoverageTemplatesReport {
                    kind: "coverage_templates",
                    version: 1,
                    rows,
                },
            };
            report::emit_collection(format, &output)
        }
        TemplatesAction::Show { name } => {
            let template = rupu_coverage::resolve_builtin(&name)
                .ok_or_else(|| anyhow::anyhow!("unknown template `{name}`"))?
                .map_err(|e| anyhow::anyhow!("template parse error: {e}"))?;
            let rows = template
                .concerns
                .iter()
                .map(|concern| ConcernRow {
                    id: concern.id.clone(),
                    severity: format!("{:?}", concern.severity),
                    name: concern.name.clone(),
                })
                .collect::<Vec<_>>();
            let output = TemplatesShowOutput {
                report: CoverageTemplateConcernsReport {
                    kind: "coverage_template_concerns",
                    version: 1,
                    rows,
                },
            };
            report::emit_collection(format, &output)
        }
    }
}

// ── catalog ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
struct CoverageCatalogReport {
    kind: &'static str,
    version: u8,
    rows: Vec<ConcernRow>,
}

struct CatalogOutput {
    report: CoverageCatalogReport,
}

impl CollectionOutput for CatalogOutput {
    type JsonReport = CoverageCatalogReport;
    type CsvRow = ConcernRow;

    fn command_name(&self) -> &'static str {
        "coverage catalog"
    }

    fn json_report(&self) -> &Self::JsonReport {
        &self.report
    }

    fn csv_rows(&self) -> &[Self::CsvRow] {
        &self.report.rows
    }

    fn csv_headers(&self) -> Option<&'static [&'static str]> {
        Some(&["id", "severity", "name"])
    }

    fn render_table(&self) -> anyhow::Result<()> {
        println!("{} concerns in effective catalog", self.report.rows.len());
        for c in &self.report.rows {
            println!("  {}  [{}]  {}", c.id, c.severity, c.name);
        }
        Ok(())
    }
}

fn run_catalog_in(workspace: &Path, target_id: &str, format: Option<OutputFormat>) -> Result<()> {
    let paths = rupu_coverage::CoveragePaths::new(workspace, target_id);
    if !paths.catalog.exists() {
        anyhow::bail!("no catalog snapshot for target `{target_id}`");
    }
    let catalog = rupu_coverage::read_snapshot(&paths.catalog)?;
    let rows = catalog
        .concerns
        .iter()
        .map(|c| ConcernRow {
            id: c.id.clone(),
            severity: format!("{:?}", c.severity),
            name: c.name.clone(),
        })
        .collect::<Vec<_>>();
    let output = CatalogOutput {
        report: CoverageCatalogReport {
            kind: "coverage_catalog",
            version: 1,
            rows,
        },
    };
    report::emit_collection(format, &output)
}

// ── show ──────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
struct CoverageShowReport {
    kind: &'static str,
    version: u8,
    files_touched: Vec<rupu_coverage::FileView>,
    assertions: Vec<rupu_coverage::ConcernAssertion>,
    findings: Vec<rupu_coverage::FindingRecord>,
}

struct CoverageShowOutput {
    report: CoverageShowReport,
}

impl DetailOutput for CoverageShowOutput {
    type JsonReport = CoverageShowReport;

    fn command_name(&self) -> &'static str {
        "coverage show"
    }

    fn supported_formats(&self) -> &'static [OutputFormat] {
        report::TABLE_JSON
    }

    fn json_report(&self) -> &Self::JsonReport {
        &self.report
    }

    fn render_human(&self) -> anyhow::Result<()> {
        let views = &self.report.files_touched;
        let assertions = &self.report.assertions;
        let findings = &self.report.findings;

        println!("== files touched ({}) ==", views.len());
        for v in views {
            println!(
                "  {}  [{}]",
                v.path,
                format!("{:?}", v.strongest).to_lowercase()
            );
        }
        println!("== concern assertions ({}) ==", assertions.len());
        for a in assertions {
            println!(
                "  {} · {} · {:?} · {}",
                a.concern_id, a.file_path, a.status, a.declared_by.model
            );
        }
        println!("== findings ({}) ==", findings.len());
        for f in findings {
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
}

fn run_show_in(workspace: &Path, target_id: &str, format: Option<OutputFormat>) -> Result<()> {
    let paths = rupu_coverage::CoveragePaths::new(workspace, target_id);
    let events = rupu_coverage::read_file_events(&paths)?;
    let views = rupu_coverage::file_views(&events);
    let assertions = rupu_coverage::read_concern_assertions(&paths)?;
    let findings = rupu_coverage::read_findings(&paths)?;

    let output = CoverageShowOutput {
        report: CoverageShowReport {
            kind: "coverage_show",
            version: 1,
            files_touched: views,
            assertions,
            findings,
        },
    };
    report::emit_detail(format, &output)
}

// ── audit ─────────────────────────────────────────────────────────────────────

struct AuditOutput {
    report: rupu_coverage::AuditReport,
}

impl DetailOutput for AuditOutput {
    type JsonReport = rupu_coverage::AuditReport;

    fn command_name(&self) -> &'static str {
        "coverage audit"
    }

    fn supported_formats(&self) -> &'static [OutputFormat] {
        report::TABLE_JSON
    }

    fn json_report(&self) -> &Self::JsonReport {
        &self.report
    }

    fn render_human(&self) -> anyhow::Result<()> {
        let report = &self.report;
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
}

fn run_audit_in(workspace: &Path, target_id: &str, format: Option<OutputFormat>) -> Result<()> {
    let paths = rupu_coverage::CoveragePaths::new(workspace, target_id);
    let report = rupu_coverage::run_audit(&paths)?;
    let output = AuditOutput { report };
    report::emit_detail(format, &output)
}

// ── gap ───────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
struct GapRow {
    concern_id: String,
    file: String,
}

#[derive(Debug, Clone, Serialize)]
struct CoverageGapsReport {
    kind: &'static str,
    version: u8,
    rows: Vec<GapRow>,
}

struct GapOutput {
    report: CoverageGapsReport,
}

impl CollectionOutput for GapOutput {
    type JsonReport = CoverageGapsReport;
    type CsvRow = GapRow;

    fn command_name(&self) -> &'static str {
        "coverage gap"
    }

    fn json_report(&self) -> &Self::JsonReport {
        &self.report
    }

    fn csv_rows(&self) -> &[Self::CsvRow] {
        &self.report.rows
    }

    fn csv_headers(&self) -> Option<&'static [&'static str]> {
        Some(&["concern_id", "file"])
    }

    fn render_table(&self) -> anyhow::Result<()> {
        if self.report.rows.is_empty() {
            println!("no coverage gaps");
            return Ok(());
        }
        // Re-group by concern_id for the human view.
        let mut concern_files: Vec<(&str, Vec<&str>)> = Vec::new();
        for row in &self.report.rows {
            match concern_files.last_mut() {
                Some((id, files)) if *id == row.concern_id.as_str() => {
                    files.push(&row.file);
                }
                _ => {
                    concern_files.push((&row.concern_id, vec![&row.file]));
                }
            }
        }
        for (concern_id, files) in &concern_files {
            println!("{} ({} gap files):", concern_id, files.len());
            for f in files {
                println!("  {f}");
            }
        }
        Ok(())
    }
}

fn run_gap_in(workspace: &Path, target_id: &str, format: Option<OutputFormat>) -> Result<()> {
    let paths = rupu_coverage::CoveragePaths::new(workspace, target_id);
    let audit_report = rupu_coverage::run_audit(&paths)?;
    let mut rows = Vec::new();
    for c in &audit_report.concerns {
        for f in &c.gap_files {
            rows.push(GapRow {
                concern_id: c.concern_id.clone(),
                file: f.clone(),
            });
        }
    }
    let output = GapOutput {
        report: CoverageGapsReport {
            kind: "coverage_gaps",
            version: 1,
            rows,
        },
    };
    report::emit_collection(format, &output)
}

// ── diff ──────────────────────────────────────────────────────────────────────

struct DiffOutput {
    target_id: String,
    diff: rupu_coverage::RunDiff,
}

impl DetailOutput for DiffOutput {
    type JsonReport = rupu_coverage::RunDiff;

    fn command_name(&self) -> &'static str {
        "coverage diff"
    }

    fn supported_formats(&self) -> &'static [OutputFormat] {
        report::TABLE_JSON
    }

    fn json_report(&self) -> &Self::JsonReport {
        &self.diff
    }

    fn render_human(&self) -> anyhow::Result<()> {
        let diff = &self.diff;
        println!(
            "coverage diff · target {} · base {} → compare {}",
            self.target_id,
            diff.base_runs.join(","),
            diff.compare_runs.join(","),
        );
        if diff.is_empty() {
            println!();
            println!("no changes between the two runs");
            return Ok(());
        }

        println!();
        println!("== cell-coverage delta ==");
        println!("  + newly asserted: {}", diff.newly_asserted.len());
        for c in &diff.newly_asserted {
            println!("      {} · {}  [{:?}]", c.concern_id, c.file_path, c.status);
        }
        println!("  - no longer asserted: {}", diff.no_longer_asserted.len());
        for c in &diff.no_longer_asserted {
            println!("      {} · {}  [{:?}]", c.concern_id, c.file_path, c.status);
        }

        if !diff.verdict_flips.is_empty() {
            println!();
            println!("== verdict flips ==");
            for f in &diff.verdict_flips {
                let mark = if f.high_signal { "!" } else { " " };
                println!(
                    "  [{}] {} · {}  {:?} → {:?}",
                    mark, f.concern_id, f.file_path, f.base_status, f.compare_status
                );
            }
        }

        if !diff.findings_appeared.is_empty() || !diff.findings_disappeared.is_empty() {
            println!();
            println!("== findings (theme-based, best-effort) ==");
            println!("  + appeared: {}", diff.findings_appeared.len());
            for f in &diff.findings_appeared {
                println!(
                    "      ({}) {}",
                    f.concern_id.as_deref().unwrap_or("-"),
                    f.theme
                );
            }
            println!("  - disappeared: {}", diff.findings_disappeared.len());
            for f in &diff.findings_disappeared {
                println!(
                    "      ({}) {}",
                    f.concern_id.as_deref().unwrap_or("-"),
                    f.theme
                );
            }
        }

        if !diff.newly_touched.is_empty() || !diff.no_longer_touched.is_empty() {
            println!();
            println!("== file-touch delta ==");
            println!("  + newly touched: {}", diff.newly_touched.len());
            for p in &diff.newly_touched {
                println!("      {p}");
            }
            println!("  - no longer touched: {}", diff.no_longer_touched.len());
            for p in &diff.no_longer_touched {
                println!("      {p}");
            }
        }
        Ok(())
    }
}

fn run_diff_in(
    workspace: &Path,
    target_id: &str,
    base: Option<String>,
    compare: Option<String>,
    format: Option<OutputFormat>,
) -> Result<()> {
    let (base, compare) = match (base, compare) {
        (None, None) => ("previous".to_string(), "latest".to_string()),
        (Some(b), Some(c)) => (b, c),
        _ => anyhow::bail!("provide both base and compare run selectors, or neither"),
    };
    // RunSelector::from_str is infallible (any non-keyword is a run id).
    let base_sel: rupu_coverage::RunSelector = base.parse().unwrap();
    let compare_sel: rupu_coverage::RunSelector = compare.parse().unwrap();

    let paths = rupu_coverage::CoveragePaths::new(workspace, target_id);
    let diff = rupu_coverage::run_diff(&paths, &base_sel, &compare_sel)?;

    let output = DiffOutput {
        target_id: target_id.to_string(),
        diff,
    };
    report::emit_detail(format, &output)
}

// ── runs ──────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
struct RunRow {
    run_id: String,
    started_at: String,
    model: String,
    surface: String,
    cells_asserted: usize,
    findings: usize,
    files_touched: usize,
}

#[derive(Debug, Clone, Serialize)]
struct CoverageRunsReport {
    kind: &'static str,
    version: u8,
    rows: Vec<rupu_coverage::RunListEntry>,
}

struct RunsOutput {
    target_id: String,
    report: CoverageRunsReport,
    csv_rows_data: Vec<RunRow>,
}

impl CollectionOutput for RunsOutput {
    type JsonReport = CoverageRunsReport;
    type CsvRow = RunRow;

    fn command_name(&self) -> &'static str {
        "coverage runs"
    }

    fn json_report(&self) -> &Self::JsonReport {
        &self.report
    }

    fn csv_rows(&self) -> &[Self::CsvRow] {
        &self.csv_rows_data
    }

    fn csv_headers(&self) -> Option<&'static [&'static str]> {
        Some(&[
            "run_id",
            "started_at",
            "model",
            "surface",
            "cells_asserted",
            "findings",
            "files_touched",
        ])
    }

    fn render_table(&self) -> anyhow::Result<()> {
        let runs = &self.report.rows;
        println!(
            "coverage runs · target {} · {} run(s)",
            self.target_id,
            runs.len()
        );
        if runs.is_empty() {
            return Ok(());
        }
        println!();
        for r in runs {
            println!(
                "  {} · {} · {:?} · {}  (cells {} / findings {} / files {})",
                r.run_id,
                r.started_at.to_rfc3339(),
                r.surface,
                r.model,
                r.cells_asserted,
                r.findings,
                r.files_touched,
            );
        }
        Ok(())
    }
}

fn run_runs_in(workspace: &Path, target_id: &str, format: Option<OutputFormat>) -> Result<()> {
    let paths = rupu_coverage::CoveragePaths::new(workspace, target_id);
    let runs = rupu_coverage::list_runs(&paths)?;

    let csv_rows_data = runs
        .iter()
        .map(|r| RunRow {
            run_id: r.run_id.clone(),
            started_at: r.started_at.to_rfc3339(),
            model: r.model.clone(),
            surface: format!("{:?}", r.surface),
            cells_asserted: r.cells_asserted,
            findings: r.findings,
            files_touched: r.files_touched,
        })
        .collect::<Vec<_>>();

    let output = RunsOutput {
        target_id: target_id.to_string(),
        report: CoverageRunsReport {
            kind: "coverage_runs",
            version: 1,
            rows: runs,
        },
        csv_rows_data,
    };
    report::emit_collection(format, &output)
}

// ── rerun ─────────────────────────────────────────────────────────────────────

async fn run_rerun_in(target_id: &str, run_id: &str) -> ExitCode {
    let ws = match workspace() {
        Ok(ws) => ws,
        Err(e) => {
            eprintln!("coverage error: {e}");
            return ExitCode::FAILURE;
        }
    };
    let paths = rupu_coverage::CoveragePaths::new(&ws, target_id);

    let manifest = match rupu_coverage::find_manifest(&paths, run_id) {
        Ok(Some(m)) => m,
        Ok(None) => {
            eprintln!(
                "coverage error: no manifest for run '{run_id}' on target '{target_id}' \
                 (runs before Slice B are not replayable)"
            );
            return ExitCode::FAILURE;
        }
        Err(e) => {
            eprintln!("coverage error: reading manifests: {e}");
            return ExitCode::FAILURE;
        }
    };

    let invocation = match rupu_coverage::plan_rerun(&manifest) {
        Ok(inv) => inv,
        Err(e) => {
            // Explicit "not yet supported" for session/workflow/autoflow.
            eprintln!("coverage error: {e}");
            return ExitCode::FAILURE;
        }
    };

    // The replay derives its target from the cwd; require the user to run
    // from the recorded workspace so the new run lands on the same target.
    if invocation.workspace_path != ws {
        eprintln!(
            "coverage error: run '{run_id}' was recorded in workspace {:?}; \
             cd there and re-run `rupu coverage rerun {target_id} {run_id}`",
            invocation.workspace_path
        );
        return ExitCode::FAILURE;
    }

    println!(
        "rerun · replaying agent '{}' on target {} …",
        invocation.agent_name, target_id
    );

    let args = crate::cmd::run::Args {
        agent: invocation.agent_name.clone(),
        target: None,
        prompt: Some(invocation.user_prompt.clone()),
        mode: Some(invocation.permission_mode.clone()),
        no_stream: false,
        view: None,
        into: None,
        tmp: false,
    };
    let code = crate::cmd::run::handle(args).await;

    println!();
    println!(
        "rerun complete · diff against the original with:\n  \
         rupu coverage diff {target_id} {run_id} latest"
    );
    code
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::output::formats::OutputFormat;

    #[test]
    fn list_in_handles_no_targets() {
        let tmp = tempfile::TempDir::new().unwrap();
        // No .rupu/coverage → prints the empty message, returns Ok.
        assert!(run_list_in(tmp.path(), None).is_ok());
    }

    #[test]
    fn templates_list_runs() {
        assert!(run_templates(TemplatesAction::List, None).is_ok());
    }

    #[test]
    fn templates_show_unknown_errors() {
        assert!(run_templates(
            TemplatesAction::Show {
                name: "nope".into()
            },
            None
        )
        .is_err());
    }

    #[test]
    fn templates_show_known_runs() {
        assert!(run_templates(
            TemplatesAction::Show {
                name: "stride".into()
            },
            None
        )
        .is_ok());
    }

    #[test]
    fn catalog_missing_snapshot_errors() {
        let tmp = tempfile::TempDir::new().unwrap();
        assert!(run_catalog_in(tmp.path(), "missing", None).is_err());
    }

    #[test]
    fn show_empty_target_is_ok() {
        let tmp = tempfile::TempDir::new().unwrap();
        // No ledger files → empty sections, no error.
        assert!(run_show_in(tmp.path(), "missing", None).is_ok());
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
        assert!(run_catalog_in(tmp.path(), "tgt", None).is_ok());
    }

    #[test]
    fn audit_on_populated_target_json_and_human() {
        use chrono::Utc;
        use rupu_coverage::{
            flatten, write_snapshot, AssertionStatus, Attribution, CatalogMode, ConcernAssertion,
            ConcernsBlock, ConcernsEntry, CoveragePaths, Evidence, FileTouchEvent,
            IncludeDirective, Surface,
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
        std::fs::write(&paths.concerns, serde_json::to_string(&a).unwrap() + "\n").unwrap();

        assert!(run_audit_in(tmp.path(), "tgt", Some(OutputFormat::Json)).is_ok()); // json
        assert!(run_audit_in(tmp.path(), "tgt", None).is_ok()); // human
        assert!(run_gap_in(tmp.path(), "tgt", None).is_ok());
    }

    #[test]
    fn diff_on_two_run_target_json_and_human() {
        use chrono::{DateTime, Utc};
        use rupu_coverage::{
            AssertionStatus, Attribution, ConcernAssertion, CoveragePaths, Evidence,
            FileTouchEvent, Surface,
        };

        let tmp = tempfile::TempDir::new().unwrap();
        let paths = CoveragePaths::new(tmp.path(), "tgt");
        paths.ensure_dir().unwrap();

        let attr = |run: &str| Attribution {
            run_id: run.to_string(),
            model: "m".to_string(),
            surface: Surface::Session,
        };
        let read = |run: &str, path: &str, secs: i64| FileTouchEvent::Read {
            path: path.to_string(),
            line_range: [1, 10],
            tool: "read_file".to_string(),
            attribution: attr(run),
            at: DateTime::<Utc>::from_timestamp(secs, 0).unwrap(),
        };
        let files = format!(
            "{}\n{}\n",
            serde_json::to_string(&read("run_old", "src/a.rs", 100)).unwrap(),
            serde_json::to_string(&read("run_new", "src/a.rs", 200)).unwrap(),
        );
        std::fs::write(&paths.files, files).unwrap();

        let mark = |run: &str, status: AssertionStatus, secs: i64| ConcernAssertion {
            concern_id: "c1".to_string(),
            file_path: "src/a.rs".to_string(),
            status,
            evidence: Evidence {
                summary: "s".to_string(),
                line_ranges: vec![],
                finding_ids: vec![],
            },
            declared_by: attr(run),
            declared_at: DateTime::<Utc>::from_timestamp(secs, 0).unwrap(),
        };
        let concerns = format!(
            "{}\n{}\n",
            serde_json::to_string(&mark("run_old", AssertionStatus::Clean, 100)).unwrap(),
            serde_json::to_string(&mark("run_new", AssertionStatus::Finding, 200)).unwrap(),
        );
        std::fs::write(&paths.concerns, concerns).unwrap();

        assert!(run_diff_in(tmp.path(), "tgt", None, None, Some(OutputFormat::Json)).is_ok());
        assert!(run_diff_in(tmp.path(), "tgt", None, None, None).is_ok());
        assert!(run_diff_in(
            tmp.path(),
            "tgt",
            Some("run_old".to_string()),
            Some("run_new".to_string()),
            None
        )
        .is_ok());
        assert!(run_diff_in(tmp.path(), "tgt", Some("run_old".to_string()), None, None).is_err());
        assert!(run_diff_in(
            tmp.path(),
            "tgt",
            Some("nope".to_string()),
            Some("run_new".to_string()),
            None
        )
        .is_err());
    }

    #[test]
    fn rerun_missing_manifest_is_detectable() {
        use rupu_coverage::CoveragePaths;
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = CoveragePaths::new(tmp.path(), "tgt");
        paths.ensure_dir().unwrap();
        // No runs.jsonl written → find_manifest returns None, which the CLI
        // surfaces as the "not replayable" error.
        assert!(rupu_coverage::find_manifest(&paths, "run_x")
            .unwrap()
            .is_none());
    }

    #[test]
    fn rerun_unsupported_surface_errors() {
        use chrono::{DateTime, Utc};
        use rupu_coverage::{
            append_manifest, find_manifest, plan_rerun, CatalogMode, ConcernsBlock, ConcernsEntry,
            CoveragePaths, IncludeDirective, RunManifest, Surface,
        };
        let tmp = tempfile::TempDir::new().unwrap();
        let paths = CoveragePaths::new(tmp.path(), "tgt");
        let m = RunManifest {
            run_id: "run_sess".to_string(),
            started_at: DateTime::<Utc>::from_timestamp(1, 0).unwrap(),
            surface: Surface::Session,
            agent_name: "a".to_string(),
            provider: "anthropic".to_string(),
            model: "m".to_string(),
            permission_mode: "bypass".to_string(),
            user_prompt: "go".to_string(),
            concerns: ConcernsBlock {
                entries: vec![ConcernsEntry::Include(IncludeDirective {
                    include: "stride".to_string(),
                    overrides: vec![],
                    mode: CatalogMode::Auto,
                    filter: None,
                })],
            },
            scope_name: "ses_1".to_string(),
            workspace_path: tmp.path().to_path_buf(),
        };
        append_manifest(&paths, &m).unwrap();
        let loaded = find_manifest(&paths, "run_sess").unwrap().unwrap();
        assert!(
            plan_rerun(&loaded).is_err(),
            "session rerun must be rejected"
        );
    }

    #[test]
    fn runs_list_json_and_human() {
        use chrono::{DateTime, Utc};
        use rupu_coverage::{
            AssertionStatus, Attribution, ConcernAssertion, CoveragePaths, Evidence, Surface,
        };

        let tmp = tempfile::TempDir::new().unwrap();
        let paths = CoveragePaths::new(tmp.path(), "tgt");
        paths.ensure_dir().unwrap();

        let a = ConcernAssertion {
            concern_id: "c1".to_string(),
            file_path: "src/a.rs".to_string(),
            status: AssertionStatus::Clean,
            evidence: Evidence {
                summary: "s".to_string(),
                line_ranges: vec![],
                finding_ids: vec![],
            },
            declared_by: Attribution {
                run_id: "run_one".to_string(),
                model: "m".to_string(),
                surface: Surface::Session,
            },
            declared_at: DateTime::<Utc>::from_timestamp(100, 0).unwrap(),
        };
        std::fs::write(&paths.concerns, serde_json::to_string(&a).unwrap() + "\n").unwrap();

        assert!(run_runs_in(tmp.path(), "tgt", Some(OutputFormat::Json)).is_ok());
        assert!(run_runs_in(tmp.path(), "tgt", None).is_ok());
        assert!(run_runs_in(tmp.path(), "tgt", Some(OutputFormat::Csv)).is_ok());

        let empty = CoveragePaths::new(tmp.path(), "empty");
        empty.ensure_dir().unwrap();
        assert!(run_runs_in(tmp.path(), "empty", None).is_ok());
    }
}
