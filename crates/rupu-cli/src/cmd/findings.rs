//! `rupu findings` — the finding report contract and report exports.
//! Thin: the schema comes from `rupu_coverage::report`, and an export is
//! collected, selected and rendered by `rupu_cp::api::findings` /
//! `rupu_findings_report`; this module parses arguments and writes the bytes.

use crate::output::formats::OutputFormat;
use crate::output::report;
use anyhow::Context;
use clap::Subcommand;
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
    /// The document format is the global `--format`: `md` (the default),
    /// `html` or `pdf`. A single `--id` with no other selection writes that
    /// finding as a stand-alone document; anything else writes one report
    /// over the selection, or with `--split` a zip holding an index and one
    /// file per finding. Findings are numbered within their project
    /// (`SEC-001`, …; see `[findings].export_id_prefix`) before the
    /// selection is applied, so a finding keeps its number whatever else is
    /// left out.
    Export(ExportArgs),
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
    /// Only findings for this CWE (e.g. CWE-79).
    #[arg(long, value_name = "CWE-N", value_parser = non_blank)]
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
    /// Where to write the report. An existing directory receives the
    /// generated file name.
    #[arg(short = 'o', long, value_name = "PATH")]
    output: PathBuf,
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

pub fn ensure_output_format(action: &Action, format: OutputFormat) -> anyhow::Result<()> {
    let (command_name, supported) = match action {
        Action::Schema { .. } => ("findings schema", report::TABLE_ONLY),
        Action::Export(_) => (
            "findings export",
            &[
                OutputFormat::Table,
                OutputFormat::Md,
                OutputFormat::Html,
                OutputFormat::Pdf,
            ][..],
        ),
    };
    crate::output::formats::ensure_supported(command_name, format, supported)
}

pub async fn handle(action: Action, format: Option<OutputFormat>) -> ExitCode {
    let result = match action {
        Action::Schema { advertised } => schema_cmd(advertised),
        Action::Export(args) => export_cmd(&args, format),
    };
    match result {
        Ok(()) => ExitCode::from(0),
        Err(e) => crate::output::diag::fail(e),
    }
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

/// The document format asked for with the global `--format`: `md` unless
/// given. Refused up front when this build cannot produce it (PDF without the
/// `pdf` feature), before any finding is read.
fn export_format(format: Option<OutputFormat>) -> anyhow::Result<Format> {
    let fmt = match format {
        None | Some(OutputFormat::Md) => Format::Markdown,
        Some(OutputFormat::Html) => Format::Html,
        Some(OutputFormat::Pdf) => Format::Pdf,
        Some(other) => anyhow::bail!(
            "findings export does not support `--format {other}` (supported: `md`, `html`, `pdf`)"
        ),
    };
    if !fmt.is_available() {
        return Err(ExportError::PdfUnavailable.into());
    }
    Ok(fmt)
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

fn export_cmd(args: &ExportArgs, format: Option<OutputFormat>) -> anyhow::Result<()> {
    let fmt = export_format(format)?;
    let global = crate::paths::global_dir()?;
    let cfg = crate::cmd::update::load_cli_config();
    let prefix = cp_findings::resolve_export_prefix(cfg.findings.export_id_prefix.as_deref());
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
    std::fs::write(&dest, &exported.bytes).with_context(|| format!("write {}", dest.display()))?;
    println!("wrote {} ({} bytes)", dest.display(), exported.bytes.len());
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
            Action::Schema { .. } => unreachable!(),
        }
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
    fn blank_selectors_are_usage_errors() {
        for flag in ["--id", "--project", "--run", "--owner", "--cwe"] {
            let argv = ["harness", "export", "-o", "out", flag, "  "];
            assert!(Harness::try_parse_from(argv).is_err(), "{flag}");
        }
    }

    #[test]
    fn the_document_format_defaults_to_markdown() {
        assert_eq!(export_format(None).unwrap(), Format::Markdown);
        assert_eq!(
            export_format(Some(OutputFormat::Html)).unwrap(),
            Format::Html
        );
        // Table / json / csv describe listings, not documents.
        assert!(export_format(Some(OutputFormat::Json)).is_err());
        assert!(export_format(Some(OutputFormat::Table)).is_err());
    }

    #[cfg(feature = "pdf")]
    #[test]
    fn pdf_is_available_with_the_pdf_feature() {
        assert_eq!(export_format(Some(OutputFormat::Pdf)).unwrap(), Format::Pdf);
    }

    #[cfg(not(feature = "pdf"))]
    #[test]
    fn pdf_without_the_pdf_feature_names_the_problem() {
        let err = export_format(Some(OutputFormat::Pdf)).unwrap_err();
        assert!(err.to_string().contains("without PDF support"), "{err}");
    }

    #[test]
    fn export_accepts_the_document_formats_and_schema_only_table() {
        let export_action = Action::Export(export(&[]));
        for f in [OutputFormat::Md, OutputFormat::Html, OutputFormat::Pdf] {
            assert!(ensure_output_format(&export_action, f).is_ok(), "{f}");
        }
        assert!(ensure_output_format(&export_action, OutputFormat::Json).is_err());
        let schema = Action::Schema { advertised: false };
        assert!(ensure_output_format(&schema, OutputFormat::Table).is_ok());
        assert!(ensure_output_format(&schema, OutputFormat::Md).is_err());
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
        let _ = export_format(Some(OutputFormat::Pdf));
        panic!("unwind-check");
    }
}
