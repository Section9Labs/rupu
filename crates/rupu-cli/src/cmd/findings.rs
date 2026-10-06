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
    Ok(cp_findings::query_findings(
        &RunStore::new(global.join("runs")),
        cp_findings::collect_all_findings(&global),
        &parsed,
    ))
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
    let result = cp_findings::tag_findings_across(&global, &change, &by)?;
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
    let configured = match rupu_config::layer_files_locked(Some(&path), None) {
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
        Some(id) => cp_findings::export_finding_report(&global, &runs, id, &prefix, fmt)?,
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

/// Files larger than this are not finding reports.
const IMPORT_MAX_BYTES: usize = 4 * 1024 * 1024;

/// What became of one file named for import.
#[derive(Debug, PartialEq)]
enum Line {
    Attached(String),
    Skipped(String),
    Failed(String, Vec<String>),
}

impl Line {
    fn failed(why: impl Into<String>) -> Self {
        Line::Failed(why.into(), Vec::new())
    }
}

/// The finding a report belongs to: the one id its labelled id lines give
/// (`own`), else `--id`. Several labelled ids, a `--id` that disagrees with
/// the labelled one, or neither, fail.
fn report_id(flag: Option<&str>, own: &[String]) -> Result<String, String> {
    match (own, flag) {
        ([_, _, ..], _) => Err(format!(
            "cites several finding ids on Finding ID lines ({})",
            own.join(", ")
        )),
        ([one], Some(id)) if one != id => Err(format!("the report says {one}; --id says {id}")),
        ([one], _) => Ok(one.clone()),
        ([], Some(id)) => Ok(id.to_string()),
        ([], None) => Err("no Finding ID line; import it alone with --id".to_string()),
    }
}

/// A report file's text; anything over [`IMPORT_MAX_BYTES`] is refused
/// without being read whole.
fn read_report_file(file: &Path) -> anyhow::Result<String> {
    use std::io::Read;
    let mut bytes = Vec::new();
    std::fs::File::open(file)?
        .take(IMPORT_MAX_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > IMPORT_MAX_BYTES {
        anyhow::bail!("larger than 4 MiB");
    }
    Ok(String::from_utf8(bytes)?)
}

/// What an [`AttachOutcome`] means for the file whose report was for `id`.
/// `not_written` is the failure line for a report that would have attached
/// to a ledger that was not written.
fn outcome_line(
    id: String,
    outcome: rupu_coverage::tools::AttachOutcome,
    not_written: Option<&str>,
) -> Line {
    use rupu_coverage::tools::report_finding::ReportFindingError;
    use rupu_coverage::tools::AttachOutcome;
    match outcome {
        AttachOutcome::Attached => Line::Attached(id),
        AttachOutcome::NotWritten => {
            Line::failed(not_written.unwrap_or("the ledger was not written"))
        }
        AttachOutcome::AlreadyHasReport => Line::Skipped(format!("{id} already has a report")),
        AttachOutcome::NotFound => Line::failed(format!("no parseable finding {id} in its ledger")),
        AttachOutcome::Duplicate => {
            Line::failed(format!("{id} appears more than once in its ledger"))
        }
        AttachOutcome::Rejected(ReportFindingError::Report(v)) => Line::Failed(
            format!("{} problem(s) in the report", v.0.len()),
            v.0.iter()
                .map(|e| format!("{}: {}", e.path, e.message))
                .collect(),
        ),
        AttachOutcome::Rejected(e) => Line::failed(e.to_string()),
    }
}

fn import_cmd(args: &ImportArgs) -> anyhow::Result<()> {
    use rupu_coverage::tools::{attach_reports, AttachItem};
    use rupu_findings_report::import::{parse_report, retain_known_cross_references, Parsed};
    use std::collections::{BTreeMap, HashSet};

    let found = find_reports(&args.paths)?;
    let files = &found.files;
    if args.id.is_some() && (files.len() != 1 || args.paths.iter().any(|p| !p.is_file())) {
        anyhow::bail!("--id needs exactly one report file");
    }
    if files.is_empty() && found.unreadable.is_empty() {
        let under: Vec<String> = args.paths.iter().map(|p| p.display().to_string()).collect();
        anyhow::bail!("no Markdown reports found under {}", under.join(", "));
    }
    let global = crate::paths::global_dir()?;
    // Limits come from the global config, as for the export prefix; an
    // unreadable config falls back to the defaults with a warning.
    let cfg_path = global.join("config.toml");
    let cfg = match rupu_config::layer_files_locked(Some(&cfg_path), None) {
        Ok(c) => c.findings,
        Err(e) => {
            crate::output::diag::warn(
                &crate::output::diag::prefs_for_diag(false),
                format!(
                    "cannot read {}: {e}; using the default findings limits",
                    cfg_path.display()
                ),
            );
            Default::default()
        }
    };
    let opts = crate::findings_opts::base_options(&global, &cfg);
    let ledgers = cp_findings::finding_ledgers(&global);
    // ledger file → the ids in it. A ledger only accepts cross-references to
    // its own findings.
    let mut ids_by_ledger: BTreeMap<PathBuf, HashSet<String>> = BTreeMap::new();
    for (id, homes) in &ledgers {
        for home in homes {
            ids_by_ledger
                .entry(home.findings.clone())
                .or_default()
                .insert(id.clone());
        }
    }

    let mut lines: BTreeMap<PathBuf, Line> = BTreeMap::new();
    // Found while searching but not readable: one failed line each.
    for (path, why) in &found.unreadable {
        lines.insert(path.clone(), Line::failed(why.clone()));
    }
    // finding id → the files whose report is for it
    let mut wanted: BTreeMap<String, Vec<(PathBuf, rupu_coverage::FindingReport)>> =
        BTreeMap::new();
    for file in files {
        let parsed = read_report_file(file).and_then(|md| Ok(parse_report(&md)?));
        match parsed {
            Err(e) => {
                lines.insert(file.clone(), Line::failed(e.to_string()));
            }
            // Named with `--id`, the file was meant as that finding's report.
            Ok(Parsed::NotAReport) if args.id.is_some() => {
                lines.insert(
                    file.clone(),
                    Line::failed(
                        "not a finding report (fewer than three of the report layout's sections)",
                    ),
                );
            }
            Ok(Parsed::NotAReport) => {
                lines.insert(file.clone(), Line::Skipped("not a finding report".into()));
            }
            Ok(Parsed::Report { report, own_ids }) => {
                match report_id(args.id.as_deref(), &own_ids) {
                    Ok(id) => wanted.entry(id).or_default().push((file.clone(), report)),
                    Err(why) => {
                        lines.insert(file.clone(), Line::failed(why));
                    }
                }
            }
        }
    }

    // Group by ledger; refuse an id two files claim, or one not found in
    // exactly one ledger.
    let mut by_ledger: BTreeMap<
        PathBuf,
        (rupu_coverage::CoveragePaths, Vec<(PathBuf, AttachItem)>),
    > = BTreeMap::new();
    for (id, reports) in wanted {
        if reports.len() > 1 {
            let names: Vec<String> = reports
                .iter()
                .map(|(f, _)| f.display().to_string())
                .collect();
            for (f, _) in reports {
                lines.insert(
                    f,
                    Line::failed(format!(
                        "{} reports cite {id}: {}",
                        names.len(),
                        names.join(", ")
                    )),
                );
            }
            continue;
        }
        let (file, mut report) = reports.into_iter().next().expect("one report");
        // The same ledger may be listed twice (a workspace registered twice,
        // an id on two lines of it); only distinct ledger files count.
        let mut homes: Vec<&rupu_coverage::CoveragePaths> = Vec::new();
        for home in ledgers.get(&id).into_iter().flatten() {
            if !homes.iter().any(|h| h.findings == home.findings) {
                homes.push(home);
            }
        }
        let paths = match homes.as_slice() {
            [one] => (*one).clone(),
            [] => {
                lines.insert(
                    file,
                    Line::failed(format!("no finding {id} in any registered project")),
                );
                continue;
            }
            _ => {
                lines.insert(
                    file,
                    Line::failed(format!("{id} is in more than one ledger")),
                );
                continue;
            }
        };
        // Keep cross-references to the ledger's other findings only: one to
        // the finding the report is being attached to is a self-reference.
        let known = ids_by_ledger.entry(paths.findings.clone()).or_default();
        let own = known.remove(&id);
        retain_known_cross_references(&mut report, known);
        if own {
            known.insert(id.clone());
        }
        by_ledger
            .entry(paths.findings.clone())
            .or_insert_with(|| (paths.clone(), Vec::new()))
            .1
            .push((
                file,
                AttachItem {
                    finding_id: id,
                    report,
                },
            ));
    }

    let mut backups = Vec::new();
    for (_, (paths, items)) in by_ledger {
        let (files, items): (Vec<PathBuf>, Vec<AttachItem>) = items.into_iter().unzip();
        let ids: Vec<String> = items.iter().map(|i| i.finding_id.clone()).collect();
        // One ledger failing fails its own files and leaves the other
        // ledgers to go on. A ledger that cannot be written (it cannot be
        // locked, is a symlink, or changed under us) still has every report
        // assessed: those with problems list them, and only those that would
        // have attached fail with the write error.
        let batch = match attach_reports(&paths, items, &opts, args.dry_run) {
            Ok(b) => b,
            Err(e) => {
                for file in files {
                    lines.insert(
                        file,
                        Line::failed(format!("cannot read {}: {e}", paths.findings.display())),
                    );
                }
                continue;
            }
        };
        backups.extend(batch.backup);
        let not_written = batch
            .write_error
            .map(|e| format!("cannot update {}: {e}", paths.findings.display()));
        for ((file, id), outcome) in files.into_iter().zip(ids).zip(batch.outcomes) {
            lines.insert(file, outcome_line(id, outcome, not_written.as_deref()));
        }
    }

    let (mut attached, mut skipped, mut failed) = (0, 0, 0);
    let verb = if args.dry_run {
        "would attach"
    } else {
        "attached"
    };
    for (file, line) in &lines {
        match line {
            Line::Attached(id) => {
                attached += 1;
                println!("{verb:<12} {} → {id}", file.display());
            }
            Line::Skipped(why) => {
                skipped += 1;
                println!("{:<12} {}: {why}", "skipped", file.display());
            }
            Line::Failed(why, details) => {
                failed += 1;
                println!("{:<12} {}: {why}", "failed", file.display());
                for d in details {
                    println!("{:<12}   {d}", "");
                }
            }
        }
    }
    for b in &backups {
        println!("{:<12} {}", "backup", b.display());
    }
    println!("{attached} {verb}, {skipped} skipped, {failed} failed");
    if failed > 0 {
        anyhow::bail!("{failed} of {} file(s) were not imported", lines.len());
    }
    Ok(())
}

/// What a search for report files found.
#[derive(Debug, Default)]
struct Found {
    /// The report files, in path order, each file once however it was named.
    files: Vec<PathBuf>,
    /// Paths found while searching a directory that could not be read, with
    /// the OS error. One failed line each; the search went on without them.
    unreadable: Vec<(PathBuf, String)>,
}

/// The files named, plus every `*.md` under the directories named, in path
/// order. While searching, hidden files and directories are skipped and
/// symlinks are not followed (a symlinked file or directory is not found).
///
/// A named path that does not exist (or cannot be looked up), and a named
/// directory that cannot be listed, fail the search, naming the path and the
/// OS error. A path found *while searching* that cannot be read is recorded
/// in [`Found::unreadable`] and the search goes on. A named *file* is only
/// looked up here: one that cannot be read fails when it is read, as its own
/// failed line.
fn find_reports(paths: &[PathBuf]) -> anyhow::Result<Found> {
    fn walk(dir: &Path, found: &mut Found) {
        match std::fs::read_dir(dir) {
            Ok(entries) => walk_entries(dir, entries, found),
            Err(e) => found.unreadable.push((dir.to_path_buf(), e.to_string())),
        }
    }
    fn walk_entries(dir: &Path, entries: std::fs::ReadDir, found: &mut Found) {
        for entry in entries {
            let entry = match entry {
                Ok(e) => e,
                // The entry itself is unknown: it is the directory that
                // could not be listed in full.
                Err(e) => {
                    found.unreadable.push((dir.to_path_buf(), e.to_string()));
                    continue;
                }
            };
            if entry.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            let path = entry.path();
            let kind = match entry.file_type() {
                Ok(k) => k,
                Err(e) => {
                    found.unreadable.push((path, e.to_string()));
                    continue;
                }
            };
            if kind.is_dir() {
                walk(&path, found);
            } else if kind.is_file()
                && path
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("md"))
            {
                found.files.push(path);
            }
        }
    }
    let mut found = Found::default();
    for p in paths {
        let meta = std::fs::metadata(p)
            .map_err(|e| anyhow::anyhow!("cannot read {}: {e}", p.display()))?;
        if meta.is_dir() {
            let entries = std::fs::read_dir(p)
                .map_err(|e| anyhow::anyhow!("cannot read {}: {e}", p.display()))?;
            walk_entries(p, entries, &mut found);
        } else {
            found.files.push(p.clone());
        }
    }
    // One file however it was spelled (`a.md` and `./a.md`, a directory and
    // a file inside it): the first spelling found is the one kept.
    let mut seen = std::collections::HashSet::new();
    found
        .files
        .retain(|f| seen.insert(std::fs::canonicalize(f).unwrap_or_else(|_| f.clone())));
    found.files.sort();
    found.unreadable.sort();
    found.unreadable.dedup();
    Ok(found)
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
    fn a_report_belongs_to_its_labelled_id_else_the_flagged_one() {
        let own = |ids: &[&str]| ids.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(report_id(None, &own(&["fnd_1"])).unwrap(), "fnd_1");
        // `--id` that agrees, or names the finding of a report with no id
        // line.
        assert_eq!(report_id(Some("fnd_1"), &own(&["fnd_1"])).unwrap(), "fnd_1");
        assert_eq!(report_id(Some("fnd_9"), &[]).unwrap(), "fnd_9");
        // `--id` never overrides the report's own id line.
        assert_eq!(
            report_id(Some("fnd_9"), &own(&["fnd_1"])).unwrap_err(),
            "the report says fnd_1; --id says fnd_9"
        );
        assert_eq!(
            report_id(None, &[]).unwrap_err(),
            "no Finding ID line; import it alone with --id"
        );
        for flag in [None, Some("fnd_1")] {
            assert_eq!(
                report_id(flag, &own(&["fnd_1", "fnd_2"])).unwrap_err(),
                "cites several finding ids on Finding ID lines (fnd_1, fnd_2)"
            );
        }
    }

    /// A path whose mode is changed for a test and put back when dropped, so
    /// the temp dir can be cleaned up even when an assertion fails first.
    #[cfg(unix)]
    struct Mode {
        path: PathBuf,
        original: std::fs::Permissions,
    }

    #[cfg(unix)]
    impl Mode {
        fn set(path: &Path, mode: u32) -> Self {
            use std::os::unix::fs::PermissionsExt;
            let original = std::fs::metadata(path).unwrap().permissions();
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
            Self {
                path: path.to_path_buf(),
                original,
            }
        }
    }

    #[cfg(unix)]
    impl Drop for Mode {
        fn drop(&mut self) {
            let _ = std::fs::set_permissions(&self.path, self.original.clone());
        }
    }

    fn names(root: &Path, files: &[PathBuf]) -> Vec<String> {
        files
            .iter()
            .map(|p| p.strip_prefix(root).unwrap().to_str().unwrap().to_string())
            .collect()
    }

    #[test]
    fn find_reports_walks_directories_for_markdown_in_path_order() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("sub/deeper")).unwrap();
        std::fs::create_dir_all(root.join(".hidden")).unwrap();
        for f in [
            "b.md",
            "a.MD",
            "notes.txt",
            "sub/c.md",
            "sub/deeper/d.md",
            ".hidden/e.md",
            ".dot.md",
        ] {
            std::fs::write(root.join(f), "x").unwrap();
        }
        let found = find_reports(&[root.to_path_buf()]).unwrap();
        assert_eq!(
            names(root, &found.files),
            ["a.MD", "b.md", "sub/c.md", "sub/deeper/d.md"]
        );
        assert!(found.unreadable.is_empty());
        // A file named outright is taken as is (whatever its extension, and
        // even a hidden one); naming it twice, or its directory as well,
        // lists it once.
        let txt = root.join("notes.txt");
        let hidden = root.join(".dot.md");
        let found =
            find_reports(&[txt.clone(), root.join("sub"), txt.clone(), hidden.clone()]).unwrap();
        assert_eq!(found.files.len(), 4, "{:?}", found.files);
        assert!(found.files.contains(&txt) && found.files.contains(&hidden));
    }

    #[test]
    fn a_file_is_listed_once_however_it_is_spelled() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::write(root.join("a.md"), "x").unwrap();
        let found = find_reports(&[
            root.join("a.md"),
            root.join("sub").join("..").join("a.md"),
            root.join(".").join("a.md"),
            root.to_path_buf(),
            root.join("sub").join("..").to_path_buf(),
        ])
        .unwrap();
        assert_eq!(found.files.len(), 1, "{:?}", found.files);
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_found_while_searching_are_not_followed() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("reports");
        let elsewhere = tmp.path().join("elsewhere");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&elsewhere).unwrap();
        std::fs::write(root.join("real.md"), "x").unwrap();
        std::fs::write(elsewhere.join("outside.md"), "x").unwrap();
        std::os::unix::fs::symlink(elsewhere.join("outside.md"), root.join("linked.md")).unwrap();
        std::os::unix::fs::symlink(&elsewhere, root.join("linked_dir")).unwrap();
        // A link back to the directory being searched must not loop.
        std::os::unix::fs::symlink(&root, root.join("loop")).unwrap();

        let found = find_reports(std::slice::from_ref(&root)).unwrap();
        assert_eq!(names(&root, &found.files), ["real.md"]);
        assert!(found.unreadable.is_empty());
        // Named outright, a link is just the file it points to.
        let found = find_reports(&[root.join("linked.md")]).unwrap();
        assert_eq!(found.files, [root.join("linked.md")]);
    }

    #[test]
    fn a_named_path_that_cannot_be_read_names_the_path_and_the_cause() {
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("missing");
        let err = find_reports(&[tmp.path().to_path_buf(), missing.clone()]).unwrap_err();
        let text = err.to_string();
        assert!(text.contains(&missing.display().to_string()), "{text}");
        assert!(text.contains("No such file or directory"), "{text}");
    }

    #[cfg(unix)]
    #[test]
    fn an_unreadable_path_found_while_searching_is_recorded_and_the_search_goes_on() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("locked")).unwrap();
        std::fs::write(root.join("a.md"), "x").unwrap();
        std::fs::write(root.join("locked/b.md"), "x").unwrap();
        let _mode = Mode::set(&root.join("locked"), 0o000);
        if std::fs::read_dir(root.join("locked")).is_ok() {
            eprintln!("skipped: directory permissions are not enforced for this user");
            return;
        }

        let found = find_reports(&[root.to_path_buf()]).unwrap();
        assert_eq!(names(root, &found.files), ["a.md"]);
        assert_eq!(found.unreadable.len(), 1, "{:?}", found.unreadable);
        assert_eq!(found.unreadable[0].0, root.join("locked"));
        assert!(
            found.unreadable[0].1.contains("Permission denied"),
            "{:?}",
            found.unreadable
        );
        // The same directory named outright fails the search, with its path
        // and the cause.
        let err = find_reports(&[root.join("locked")])
            .unwrap_err()
            .to_string();
        assert!(
            err.contains(&root.join("locked").display().to_string()),
            "{err}"
        );
        assert!(err.contains("Permission denied"), "{err}");
    }

    #[test]
    fn an_oversized_report_file_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let big = tmp.path().join("big.md");
        std::fs::write(&big, vec![b'a'; IMPORT_MAX_BYTES + 1]).unwrap();
        let err = read_report_file(&big).unwrap_err();
        assert!(err.to_string().contains("larger than 4 MiB"), "{err}");
        let ok = tmp.path().join("ok.md");
        std::fs::write(&ok, vec![b'a'; IMPORT_MAX_BYTES]).unwrap();
        assert_eq!(read_report_file(&ok).unwrap().len(), IMPORT_MAX_BYTES);
        let bin = tmp.path().join("bin.md");
        std::fs::write(&bin, [0xff, 0xfe]).unwrap();
        assert!(read_report_file(&bin).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn an_unreadable_report_file_reports_the_os_error() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("locked.md");
        std::fs::write(&file, "x").unwrap();
        let _mode = Mode::set(&file, 0o000);
        if std::fs::File::open(&file).is_ok() {
            eprintln!("skipped: file permissions are not enforced for this user");
            return;
        }
        let err = read_report_file(&file).unwrap_err().to_string();
        assert!(err.contains("Permission denied"), "{err}");
    }

    #[test]
    fn each_attach_outcome_has_its_wording() {
        use rupu_coverage::report::{ArtifactError, FieldError, ReportValidationError};
        use rupu_coverage::tools::report_finding::ReportFindingError;
        use rupu_coverage::tools::AttachOutcome;
        let id = || "fnd_1".to_string();
        assert_eq!(
            outcome_line(id(), AttachOutcome::Attached, None),
            Line::Attached(id())
        );
        assert_eq!(
            outcome_line(id(), AttachOutcome::AlreadyHasReport, None),
            Line::Skipped("fnd_1 already has a report".into())
        );
        assert_eq!(
            outcome_line(id(), AttachOutcome::NotFound, None),
            Line::failed("no parseable finding fnd_1 in its ledger")
        );
        assert_eq!(
            outcome_line(id(), AttachOutcome::Duplicate, None),
            Line::failed("fnd_1 appears more than once in its ledger")
        );
        let problems = ReportValidationError(vec![
            FieldError {
                path: "report.artifacts[0].path".into(),
                message: "must not contain `..`".into(),
            },
            FieldError {
                path: "report.impact".into(),
                message: "must not be empty".into(),
            },
        ]);
        assert_eq!(
            outcome_line(
                id(),
                AttachOutcome::Rejected(ReportFindingError::Report(problems)),
                None
            ),
            Line::Failed(
                "2 problem(s) in the report".into(),
                vec![
                    "report.artifacts[0].path: must not contain `..`".into(),
                    "report.impact: must not be empty".into(),
                ]
            )
        );
        assert_eq!(
            outcome_line(
                id(),
                AttachOutcome::Rejected(ReportFindingError::Artifact(ArtifactError::Missing {
                    path: "out/x.txt".into()
                })),
                None
            ),
            Line::failed("artifact `out/x.txt` does not exist in the workspace")
        );
        // A report that would have attached to a ledger that was not written
        // fails with the write error.
        assert_eq!(
            outcome_line(
                id(),
                AttachOutcome::NotWritten,
                Some("cannot update l: locked")
            ),
            Line::failed("cannot update l: locked")
        );
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
