//! `rupu findings` — the finding report contract, report exports, the
//! one-time import of reports written before the full profile, and finding
//! tags (list/tag/tags).
//! Thin: the schema comes from `rupu_coverage::report`, an export is
//! collected, selected and rendered by `rupu_cp::api::findings` /
//! `rupu_findings_report`, and an import is parsed by
//! `rupu_findings_report::import` and written by
//! `rupu_coverage::tools::attach_reports`; this module parses arguments,
//! moves bytes and prints.

use crate::output::formats::OutputFormat;
use crate::output::report;
use anyhow::Context;
use clap::{Subcommand, ValueEnum};
use rupu_cp::api::findings as cp_findings;
use rupu_findings_report::{ExportError, Format};
use rupu_orchestrator::runs::RunStore;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

#[derive(Debug, Subcommand)]
pub enum Action {
    /// Print the finding report JSON Schema embedded in this build.
    Schema {
        /// Print the simplified copy used in tool definitions instead.
        #[arg(long)]
        advertised: bool,
    },
    /// Export findings as a report: one finding, or a project report.
    ///
    /// A single `--id` with no other selection writes that finding as a
    /// stand-alone document; anything else writes one report over the
    /// selection, or with `--split` a zip holding an index and one file per
    /// finding. Findings are numbered within their project (`SEC-001`, …)
    /// before the selection is applied, so a finding keeps its number
    /// whatever else is left out. The number prefix is
    /// `[findings].export_id_prefix` in the GLOBAL config (`~/.rupu/config.toml`);
    /// a project's `.rupu/config.toml` never changes it.
    Export(ExportArgs),
    /// Attach reports written before the full profile to their findings.
    ///
    /// A one-time migration aid: reads Markdown finding reports in the report
    /// layout, and attaches each to the summary finding its `Finding ID:`
    /// line names (also read: `Native Finding:`, `Rupu Finding ID:` and
    /// similar labels; an id mentioned only in prose is never used). The
    /// finding becomes a full-profile finding. A report attaches whole or not
    /// at all; a finding that already has a report is left alone; each
    /// changed ledger is backed up first (`findings.jsonl.pre-import-<time>`).
    Import(ImportArgs),
    /// List findings matching a query, e.g. `severity>=high -tag:noise`.
    ///
    /// Put flags (`--ids-only`, `--limit`) before the query words.
    List(ListArgs),
    /// Add or remove tags on findings.
    ///
    /// Tags are free-form: lowercase a-z, 0-9 and `. _ : / -`, starting with
    /// a letter or digit, at most 64 characters (`Needs-POC` is stored as
    /// `needs-poc`). Give `-` as the only id to read ids from stdin, one per
    /// line: `rupu findings list --ids-only tag:class:sqli | rupu findings
    /// tag - --add needs-poc`. Findings in different projects are changed
    /// project by project; an unknown id fails the command after the rest
    /// are changed.
    Tag(TagArgs),
    /// List the tags in use, with how many findings carry each (optionally
    /// only on the findings a query selects).
    Tags(TagsArgs),
}

#[derive(Debug, clap::Args)]
pub struct ExportArgs {
    /// Export this finding (repeatable).
    #[arg(long = "id", value_name = "FINDING_ID", value_parser = non_blank)]
    ids: Vec<String>,
    /// Only this project: a workspace id, or the path of its checkout.
    #[arg(long, value_name = "WS_ID|PATH", value_parser = non_blank)]
    project: Option<String>,
    /// Only the findings a run (and its sub-runs) declared.
    #[arg(long, value_name = "RUN_ID", value_parser = non_blank)]
    run: Option<String>,
    /// Only this severity and worse.
    #[arg(
        long,
        value_name = "SEVERITY",
        value_parser = ["critical", "high", "medium", "low", "info"]
    )]
    severity: Option<String>,
    /// Only findings whose report names this owner.
    #[arg(long, value_name = "OWNER", value_parser = non_blank)]
    owner: Option<String>,
    /// Only findings for this CWE (e.g. CWE-79), compared by number: CWE-79
    /// never selects CWE-798.
    #[arg(long, value_name = "CWE-N", value_parser = cwe_id)]
    cwe: Option<String>,
    /// Keep findings recorded without a full report (left out by default).
    #[arg(long)]
    include_summaries: bool,
    /// Write a zip of one file per finding plus an index.
    #[arg(long)]
    split: bool,
    /// Title of the project report (default: "Findings report").
    #[arg(long, value_name = "TITLE")]
    title: Option<String>,
    /// Document format.
    #[arg(long, value_enum, default_value_t = ExportFormat::Md)]
    to: ExportFormat,
    /// Where to write the report. An existing directory receives the
    /// generated file name.
    #[arg(short = 'o', long, value_name = "PATH")]
    output: PathBuf,
}

#[derive(Debug, clap::Args)]
pub struct ImportArgs {
    /// Report files, or directories to search for `*.md` files.
    #[arg(required = true, value_name = "PATH")]
    paths: Vec<PathBuf>,
    /// The finding a single report belongs to, for a report with no
    /// `Finding ID:` line. Only with one file; refused when the report's own
    /// `Finding ID:` line names a different finding.
    #[arg(long = "id", value_name = "FINDING_ID", value_parser = non_blank)]
    id: Option<String>,
    /// Parse and validate every report, check that the artifacts it lists
    /// exist and that it is within the size limits once they are recorded;
    /// write nothing (no ledger change, backup or lock file). No artifact is
    /// copied (only its first 8 KiB is read), so trouble found only while
    /// copying one shows up only on a real import.
    #[arg(long)]
    dry_run: bool,
}

#[derive(Debug, clap::Args)]
pub struct ListArgs {
    /// Findings query, e.g. `severity>=high tag:class:sqli -tag:false-positive`.
    /// Several words are joined with spaces; quote anything with `>`, `<` or
    /// spaces for your shell. Keys: severity (>=,>,<=,<), tag, has, project,
    /// cwe, owner, product, verified, profile, scope, concern, agent,
    /// workflow, file, run, id; bare words search title/summary/id/file.
    /// Put flags before the query words.
    #[arg(value_name = "QUERY", allow_hyphen_values = true, num_args = 0..)]
    query: Vec<String>,
    /// Show at most N findings; how many were left out goes to stderr.
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u64).range(1..))]
    limit: Option<u64>,
    /// Print only the finding ids, one per line.
    #[arg(long)]
    ids_only: bool,
}

#[derive(Debug, clap::Args)]
pub struct TagArgs {
    /// Finding ids, or `-` alone to read them from stdin.
    #[arg(required = true, value_name = "FINDING_ID")]
    ids: Vec<String>,
    /// Tag to add (repeatable).
    #[arg(long = "add", value_name = "TAG", value_parser = tag_arg)]
    add: Vec<rupu_coverage::Tag>,
    /// Tag to remove (repeatable).
    #[arg(long = "remove", value_name = "TAG", value_parser = tag_arg)]
    remove: Vec<rupu_coverage::Tag>,
}

#[derive(Debug, clap::Args)]
pub struct TagsArgs {
    /// Count only the tags on findings this query selects (same syntax as
    /// `findings list`).
    #[arg(value_name = "QUERY", allow_hyphen_values = true, num_args = 0..)]
    query: Vec<String>,
}

fn tag_arg(raw: &str) -> Result<rupu_coverage::Tag, String> {
    rupu_coverage::Tag::parse(raw).map_err(|e| e.to_string())
}

/// The document formats `--to` accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum ExportFormat {
    Md,
    Html,
    Pdf,
}

impl ExportArgs {
    /// The one finding to write as a stand-alone document: a single `--id`
    /// and nothing that only makes sense for a project report.
    fn single_id(&self) -> Option<&str> {
        let [id] = self.ids.as_slice() else {
            return None;
        };
        let narrowed = self.project.is_some()
            || self.run.is_some()
            || self.severity.is_some()
            || self.owner.is_some()
            || self.cwe.is_some()
            || self.split
            || self.title.is_some();
        (!narrowed).then_some(id.as_str())
    }
}

/// Reject an empty or all-whitespace value, which would silently match
/// nothing (or everything).
fn non_blank(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        Err("must not be empty".to_string())
    } else {
        Ok(trimmed.to_string())
    }
}

/// A `--cwe` value (`CWE-79`, `cwe-79` or `79`), normalised to `CWE-79`.
/// Anything else is a usage error rather than a filter that matches nothing.
fn cwe_id(raw: &str) -> Result<String, String> {
    let raw = non_blank(raw)?;
    rupu_findings_report::select::parse_cwe(&raw)
        .map(|n| format!("CWE-{n}"))
        .ok_or_else(|| format!("`{raw}` is not a CWE id (expected e.g. CWE-79)"))
}

pub fn ensure_output_format(action: &Action, format: OutputFormat) -> anyhow::Result<()> {
    // Every action writes a schema / a file / a status listing: the global
    // `--format` (table, json, csv, …) has nothing to shape. The document
    // format is `--to`.
    let command_name = match action {
        Action::Schema { .. } => "findings schema",
        Action::Export(_) => "findings export",
        Action::Import(_) => "findings import",
        Action::List(_) => "findings list",
        Action::Tag(_) => "findings tag",
        Action::Tags(_) => "findings tags",
    };
    let supported = match action {
        Action::List(_) | Action::Tag(_) | Action::Tags(_) => report::TABLE_JSON,
        _ => report::TABLE_ONLY,
    };
    crate::output::formats::ensure_supported(command_name, format, supported)
}

pub async fn handle(action: Action, format: Option<OutputFormat>) -> ExitCode {
    let json = matches!(format, Some(OutputFormat::Json));
    let result = match action {
        Action::Schema { advertised } => schema_cmd(advertised),
        Action::Export(args) => export_cmd(&args),
        Action::Import(args) => import_cmd(&args),
        Action::List(args) => list_cmd(&args, json),
        Action::Tag(args) => tag_cmd(&args, json),
        Action::Tags(args) => tags_cmd(&args, json),
    };
    match result {
        Ok(()) => ExitCode::from(0),
        Err(e) => crate::output::diag::fail(e),
    }
}

/// Every finding the query words select, with provenance.
fn queried_findings(query: &[String]) -> anyhow::Result<Vec<cp_findings::FindingOut>> {
    // A trailing flag would parse as a negated free-text word: refuse it.
    if let Some(word) = query.iter().find(|w| w.starts_with("--")) {
        anyhow::bail!(
            "`{word}` looks like a flag — put flags (like --ids-only, --limit) before the query words"
        );
    }
    let parsed = rupu_coverage::parse_query(&query.join(" "))?;
    let global = crate::paths::global_dir()?;
    let runs = RunStore::new(global.join("runs"));
    let mut findings = cp_findings::collect_all_findings(&global);
    cp_findings::join_workflow_names(&runs, &mut findings);
    Ok(cp_findings::query_findings(&global, &runs, findings, &parsed))
}

#[derive(serde::Serialize)]
struct ListRow {
    #[serde(flatten)]
    row: rupu_coverage::FindingRow,
    ws_id: String,
    project: String,
}

fn tag_list(tags: &[rupu_coverage::Tag]) -> String {
    if tags.is_empty() {
        "(none)".to_string()
    } else {
        tags.iter()
            .map(|t| t.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

fn list_cmd(args: &ListArgs, json: bool) -> anyhow::Result<()> {
    let selected = queried_findings(&args.query)?;
    let total = selected.len();
    let shown: Vec<&cp_findings::FindingOut> = match args.limit {
        Some(n) => selected.iter().take(n as usize).collect(),
        None => selected.iter().collect(),
    };
    if shown.len() < total {
        eprintln!("showing {} of {total} findings", shown.len());
    }
    if args.ids_only {
        for f in &shown {
            println!("{}", f.record.id);
        }
        return Ok(());
    }
    let rows: Vec<ListRow> = shown
        .iter()
        .map(|f| ListRow {
            row: rupu_coverage::FindingRow::from(&f.record),
            ws_id: f.ws_id.clone(),
            project: f.project.clone(),
        })
        .collect();
    if json {
        return crate::output::formats::print_json(&rows);
    }
    if rows.is_empty() {
        println!("no findings match");
        return Ok(());
    }
    let mut t = crate::output::tables::new_table();
    t.set_header(vec![
        "ID", "SEVERITY", "TITLE", "LOCATION", "TAGS", "PROJECT",
    ]);
    for r in &rows {
        let mut title = r.row.title.clone();
        if title.chars().count() > 60 {
            title = title.chars().take(59).collect::<String>() + "…";
        }
        t.add_row(vec![
            r.row.id.clone(),
            r.row.severity.as_str().to_string(),
            title,
            r.row.location.clone().unwrap_or_default(),
            if r.row.tags.is_empty() {
                String::new()
            } else {
                tag_list(&r.row.tags)
            },
            r.project.clone(),
        ]);
    }
    println!("{t}");
    Ok(())
}

/// The ids to change: the arguments, or stdin's lines when `-` is the only one.
fn finding_ids(args: &[String], stdin: impl std::io::BufRead) -> anyhow::Result<Vec<String>> {
    if args.len() == 1 && args[0] == "-" {
        let mut ids = Vec::new();
        for line in stdin.lines() {
            let line = line?;
            let id = line.trim();
            if !id.is_empty() {
                ids.push(id.to_string());
            }
        }
        if ids.is_empty() {
            anyhow::bail!("no finding ids on stdin");
        }
        return Ok(ids);
    }
    if args.iter().any(|a| a == "-") {
        anyhow::bail!("`-` (read ids from stdin) must be the only id");
    }
    Ok(args.to_vec())
}

fn tag_cmd(args: &TagArgs, json: bool) -> anyhow::Result<()> {
    let change = rupu_coverage::TagChange {
        finding_ids: finding_ids(&args.ids, std::io::stdin().lock())?,
        add: args.add.clone(),
        remove: args.remove.clone(),
    };
    let global = crate::paths::global_dir()?;
    let by = rupu_coverage::TagActor::operator(rupu_coverage::OperatorSurface::Cli);
    let result = cp_findings::tag_findings_across(&global, &change, &by, None)?;
    if json {
        crate::output::formats::print_json(&result)?;
    } else {
        for w in &result.workspaces {
            if let Some(e) = &w.error {
                println!("{}: not changed: {e}", w.ws_id);
                continue;
            }
            for o in &w.outcomes {
                if o.changed() {
                    println!(
                        "{}: {} → {}",
                        o.finding_id,
                        tag_list(&o.before),
                        tag_list(&o.after)
                    );
                } else {
                    println!("{}: {} (unchanged)", o.finding_id, tag_list(&o.after));
                }
            }
        }
    }
    let failed: Vec<&str> = result
        .workspaces
        .iter()
        .filter(|w| w.error.is_some())
        .map(|w| w.ws_id.as_str())
        .collect();
    if result.unknown.is_empty() && failed.is_empty() {
        return Ok(());
    }
    let mut why = Vec::new();
    if !result.unknown.is_empty() {
        why.push(format!(
            "unknown finding id(s): {}",
            result.unknown.join(", ")
        ));
    }
    if !failed.is_empty() {
        why.push(format!("nothing changed in {}", failed.join(", ")));
    }
    let partial = result
        .workspaces
        .iter()
        .any(|w| w.error.is_none() && w.outcomes.iter().any(|o| o.changed()));
    anyhow::bail!(
        "{}{}",
        why.join("; "),
        if partial {
            " (the other findings were changed)"
        } else {
            ""
        }
    )
}

fn tags_cmd(args: &TagsArgs, json: bool) -> anyhow::Result<()> {
    let selected = queried_findings(&args.query)?;
    let counts = rupu_coverage::tags_in_use(selected.iter().map(|f| &f.record));
    if json {
        return crate::output::formats::print_json(&counts);
    }
    if counts.is_empty() {
        println!("no tags in use");
        return Ok(());
    }
    let mut t = crate::output::tables::new_table();
    t.set_header(vec!["TAG", "FINDINGS"]);
    for c in &counts {
        t.add_row(vec![c.tag.as_str().to_string(), c.count.to_string()]);
    }
    println!("{t}");
    Ok(())
}

fn schema_cmd(advertised: bool) -> anyhow::Result<()> {
    let v = if advertised {
        rupu_coverage::report::schema::advertised_schema()
    } else {
        rupu_coverage::report::schema::canonical_schema()
    };
    println!("{}", serde_json::to_string_pretty(&v)?);
    Ok(())
}

/// The document format asked for with `--to`. Refused up front when this
/// build cannot produce it (PDF without the `pdf` feature), before any
/// finding is read.
fn document_format(to: ExportFormat) -> anyhow::Result<Format> {
    let fmt = match to {
        ExportFormat::Md => Format::Markdown,
        ExportFormat::Html => Format::Html,
        ExportFormat::Pdf => Format::Pdf,
    };
    if !fmt.is_available() {
        return Err(ExportError::PdfUnavailable.into());
    }
    Ok(fmt)
}

/// The display-number prefix, from the GLOBAL config only. A project's
/// `.rupu/config.toml` is deliberately not layered in (unlike most commands),
/// and this is the same resolution `rupu cp serve` uses, so a finding is
/// numbered identically wherever it is exported. A global config that cannot
/// be read falls back to the default prefix with a warning (as the control
/// plane does).
fn export_prefix(global: &Path) -> String {
    let path = global.join("config.toml");
    let layers = rupu_config::LayerPaths::global_only(&path);
    let configured = match rupu_config::layer_files_locked(layers) {
        Ok(cfg) => cfg.findings.export_id_prefix,
        Err(e) => {
            crate::output::diag::warn(
                &crate::output::diag::prefs_for_diag(false),
                format!(
                    "cannot read {}: {e}; numbering findings with the default prefix",
                    path.display()
                ),
            );
            None
        }
    };
    cp_findings::resolve_export_prefix(configured.as_deref())
}

/// Where the report goes: `output` itself, or inside it when it is an
/// existing directory.
fn destination(output: &Path, generated_name: &str) -> PathBuf {
    if output.is_dir() {
        output.join(generated_name)
    } else {
        output.to_path_buf()
    }
}

/// Warn (never fail) when `dest`'s extension is not the one of what is being
/// written, e.g. a zip going to `x.pdf`.
fn warn_on_extension_mismatch(dest: &Path, generated_name: &str) {
    let ext_of = |p: &str| {
        Path::new(p)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase()
    };
    let expected = ext_of(generated_name);
    let actual = ext_of(&dest.to_string_lossy());
    let same = expected == actual
        || (expected == "md" && actual == "markdown")
        || (expected == "html" && actual == "htm");
    if same {
        return;
    }
    let what = match expected.as_str() {
        "zip" => "a zip archive",
        "md" => "a Markdown report",
        "html" => "an HTML report",
        "pdf" => "a PDF report",
        _ => "a report",
    };
    crate::output::diag::warn(
        &crate::output::diag::prefs_for_diag(false),
        format!(
            "writing {what} to {} (expected .{expected})",
            dest.display()
        ),
    );
}

fn export_cmd(args: &ExportArgs) -> anyhow::Result<()> {
    let fmt = document_format(args.to)?;
    let global = crate::paths::global_dir()?;
    let prefix = export_prefix(&global);
    let runs = RunStore::new(global.join("runs"));

    let exported = match args.single_id() {
        Some(id) => {
            // `--project` picks the workspace when the id exists in several.
            let ws_id = args
                .project
                .as_deref()
                .map(|project| cp_findings::resolve_project(&global, project))
                .transpose()
                .map_err(anyhow::Error::msg)?;
            cp_findings::export_finding_report(&global, &runs, id, ws_id.as_deref(), &prefix, fmt)
                .map_err(|e| match e {
                cp_findings::ReportError::Ambiguous { .. } => {
                    anyhow::anyhow!("{e}: pass --project to pick one")
                }
                other => other.into(),
            })?
        }
        None => {
            let ws_id = args
                .project
                .as_deref()
                .map(|project| cp_findings::resolve_project(&global, project))
                .transpose()
                .map_err(anyhow::Error::msg)?;
            let min_severity = args
                .severity
                .as_deref()
                .map(|raw| {
                    cp_findings::parse_min_severity(raw)
                        .with_context(|| format!("unknown severity `{raw}`"))
                })
                .transpose()?;
            let title = cp_findings::normalize_export_title(args.title.as_deref())
                .map_err(anyhow::Error::msg)?;
            let req = cp_findings::ReportRequest {
                title,
                ids: args.ids.clone(),
                ws_id,
                run_id: args.run.clone(),
                min_severity,
                owner: args.owner.clone(),
                cwe: args.cwe.clone(),
                include_summaries: args.include_summaries,
                split: args.split,
            };
            cp_findings::export_project_report(&global, &runs, &prefix, req, fmt)?
        }
    };

    let dest = destination(&args.output, &exported.name);
    warn_on_extension_mismatch(&dest, &exported.name);
    // Spelt out (rather than `.context()`): the failure line shows only the
    // outermost message, and the OS error is the part that says what to fix.
    std::fs::write(&dest, &exported.bytes)
        .map_err(|e| anyhow::anyhow!("cannot write {}: {e}", dest.display()))?;
    println!("wrote {} ({} bytes)", dest.display(), exported.bytes.len());
    Ok(())
}

fn import_cmd(args: &ImportArgs) -> anyhow::Result<()> {
    use rupu_findings_report::import_files::{FileOutcome, ImportPlan, ImportRequest};
    let plan = ImportPlan::find(ImportRequest {
        paths: args.paths.clone(),
        id: args.id.clone(),
        dry_run: args.dry_run,
        id_flag: "--id",
        max_entries: None,
    })?;
    let global = crate::paths::global_dir()?;
    let (opts, warning) = rupu_runtime::findings_opts::global_options(&global);
    if let Some(w) = warning {
        crate::output::diag::warn(&crate::output::diag::prefs_for_diag(false), w);
    }
    let report = plan.run(&cp_findings::finding_ledgers(&global), &opts);
    let verb = if args.dry_run {
        "would attach"
    } else {
        "attached"
    };
    for file in &report.files {
        let path = file.path.display();
        match &file.outcome {
            FileOutcome::Attached { finding_id } => println!("{verb:<12} {path} → {finding_id}"),
            FileOutcome::Skipped { reason } => println!("{:<12} {path}: {reason}", "skipped"),
            FileOutcome::Failed { reason, details } => {
                println!("{:<12} {path}: {reason}", "failed");
                for d in details {
                    println!("{:<12}   {d}", "");
                }
            }
        }
    }
    for b in &report.backups {
        println!("{:<12} {}", "backup", b.display());
    }
    println!(
        "{} {verb}, {} skipped, {} failed",
        report.attached, report.skipped, report.failed
    );
    if report.failed > 0 {
        anyhow::bail!(
            "{} of {} file(s) were not imported",
            report.failed,
            report.files.len()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Parser, Debug)]
    struct Harness {
        #[command(subcommand)]
        action: Action,
    }

    fn export(args: &[&str]) -> ExportArgs {
        let argv = ["harness", "export", "-o", "out"]
            .into_iter()
            .chain(args.iter().copied());
        match Harness::try_parse_from(argv).unwrap().action {
            Action::Export(a) => a,
            _ => unreachable!(),
        }
    }

    fn import(args: &[&str]) -> Result<ImportArgs, clap::Error> {
        let argv = ["harness", "import"]
            .into_iter()
            .chain(args.iter().copied());
        Harness::try_parse_from(argv).map(|h| match h.action {
            Action::Import(a) => a,
            _ => unreachable!(),
        })
    }

    #[test]
    fn one_id_alone_is_a_single_finding_document() {
        assert_eq!(export(&["--id", "fnd_1"]).single_id(), Some("fnd_1"));
        // Asking for a summary finding's data changes nothing about the shape.
        assert_eq!(
            export(&["--id", "fnd_1", "--include-summaries"]).single_id(),
            Some("fnd_1")
        );
    }

    #[test]
    fn anything_beyond_one_id_is_a_project_report() {
        assert_eq!(export(&[]).single_id(), None);
        assert_eq!(export(&["--id", "a", "--id", "b"]).single_id(), None);
        for extra in [
            ["--project", "ws1"],
            ["--run", "run_1"],
            ["--severity", "high"],
            ["--owner", "team"],
            ["--cwe", "CWE-79"],
            ["--title", "Q3"],
        ] {
            let mut args = vec!["--id", "fnd_1"];
            args.extend(extra);
            assert_eq!(export(&args).single_id(), None, "{extra:?}");
        }
        assert_eq!(export(&["--id", "fnd_1", "--split"]).single_id(), None);
    }

    #[test]
    fn a_cwe_is_normalised_and_an_unreadable_one_is_a_usage_error() {
        for raw in ["CWE-79", "cwe-79", "79"] {
            assert_eq!(export(&["--cwe", raw]).cwe.as_deref(), Some("CWE-79"));
        }
        for bad in ["xss", "CWE-", "CWE-79a"] {
            let argv = ["harness", "export", "-o", "out", "--cwe", bad];
            assert!(Harness::try_parse_from(argv).is_err(), "{bad}");
        }
    }

    #[test]
    fn blank_selectors_are_usage_errors() {
        for flag in ["--id", "--project", "--run", "--owner", "--cwe"] {
            let argv = ["harness", "export", "-o", "out", flag, "  "];
            assert!(Harness::try_parse_from(argv).is_err(), "{flag}");
        }
    }

    #[test]
    fn the_document_format_defaults_to_markdown() {
        assert_eq!(export(&[]).to, ExportFormat::Md);
        assert_eq!(export(&["--to", "html"]).to, ExportFormat::Html);
        assert_eq!(export(&["--to", "pdf"]).to, ExportFormat::Pdf);
        assert_eq!(document_format(ExportFormat::Md).unwrap(), Format::Markdown);
        assert_eq!(document_format(ExportFormat::Html).unwrap(), Format::Html);
        let argv = ["harness", "export", "-o", "out", "--to", "docx"];
        assert!(Harness::try_parse_from(argv).is_err());
    }

    #[cfg(feature = "pdf")]
    #[test]
    fn pdf_is_available_with_the_pdf_feature() {
        assert_eq!(document_format(ExportFormat::Pdf).unwrap(), Format::Pdf);
    }

    #[cfg(not(feature = "pdf"))]
    #[test]
    fn pdf_without_the_pdf_feature_names_the_problem() {
        let err = document_format(ExportFormat::Pdf).unwrap_err();
        assert!(err.to_string().contains("without PDF support"), "{err}");
    }

    #[test]
    fn the_global_format_flag_has_nothing_to_shape() {
        let export_action = Action::Export(export(&[]));
        let import_action = Action::Import(import(&["reports"]).unwrap());
        let schema = Action::Schema { advertised: false };
        for action in [&export_action, &import_action, &schema] {
            assert!(ensure_output_format(action, OutputFormat::Table).is_ok());
            for f in [OutputFormat::Json, OutputFormat::Csv, OutputFormat::Pretty] {
                assert!(ensure_output_format(action, f).is_err(), "{f}");
            }
        }
    }

    #[test]
    fn import_takes_paths_an_optional_id_and_a_dry_run_flag() {
        let a = import(&["a.md", "dir", "--dry-run", "--id", " fnd_1 "]).unwrap();
        assert_eq!(a.paths, [PathBuf::from("a.md"), PathBuf::from("dir")]);
        assert_eq!(a.id.as_deref(), Some("fnd_1"));
        assert!(a.dry_run);
        let a = import(&["a.md"]).unwrap();
        assert!(a.id.is_none() && !a.dry_run);
        // At least one path; a blank id is a usage error.
        assert!(import(&[]).is_err());
        assert!(import(&["a.md", "--id", "  "]).is_err());
    }

    #[test]
    fn an_existing_directory_receives_the_generated_name() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(
            destination(tmp.path(), "report.md"),
            tmp.path().join("report.md")
        );
        let file = tmp.path().join("x.md");
        assert_eq!(destination(&file, "report.md"), file);
    }

    /// Typst makes the linked `__eh_frame` very large; on macOS the linker
    /// warns that it cannot build the compact-unwind table ("performance of
    /// exception handling might be affected"). This test binary links the
    /// same code, so a broken unwinder would abort it here instead of
    /// unwinding the panic to the harness.
    #[test]
    #[should_panic(expected = "unwind-check")]
    fn panics_still_unwind_in_a_typst_linked_binary() {
        // Keep the export path (and so Typst) referenced from this binary.
        let _ = document_format(ExportFormat::Pdf);
        panic!("unwind-check");
    }
}
