use crate::output::formats::{self, OutputFormat};
use serde::Serialize;

pub const TABLE_ONLY: &[OutputFormat] = &[OutputFormat::Table];
pub const TABLE_JSON: &[OutputFormat] = &[OutputFormat::Table, OutputFormat::Json];
pub const TABLE_JSON_CSV: &[OutputFormat] =
    &[OutputFormat::Table, OutputFormat::Json, OutputFormat::Csv];
pub const PRETTY_TABLE_JSON: &[OutputFormat] = &[
    OutputFormat::Pretty,
    OutputFormat::Table,
    OutputFormat::Json,
];
pub const PRETTY_TABLE_JSON_JSONL: &[OutputFormat] = &[
    OutputFormat::Pretty,
    OutputFormat::Table,
    OutputFormat::Json,
    OutputFormat::Jsonl,
];

pub trait CollectionOutput {
    type JsonReport: Serialize;
    type CsvRow: Serialize;

    fn command_name(&self) -> &'static str;

    fn default_format(&self) -> OutputFormat {
        OutputFormat::Table
    }

    fn supported_formats(&self) -> &'static [OutputFormat] {
        TABLE_JSON_CSV
    }

    fn json_report(&self) -> &Self::JsonReport;

    fn csv_rows(&self) -> &[Self::CsvRow];

    fn csv_headers(&self) -> Option<&'static [&'static str]> {
        None
    }

    fn render_table(&self) -> anyhow::Result<()>;
}

pub trait DetailOutput {
    type JsonReport: Serialize;

    fn command_name(&self) -> &'static str;

    fn default_format(&self) -> OutputFormat {
        OutputFormat::Table
    }

    fn supported_formats(&self) -> &'static [OutputFormat] {
        TABLE_JSON
    }

    fn json_report(&self) -> &Self::JsonReport;

    fn render_human(&self) -> anyhow::Result<()>;
}

pub trait EventOutput {
    type JsonReport: Serialize;
    type JsonlRow: Serialize;

    fn command_name(&self) -> &'static str;

    fn default_format(&self) -> OutputFormat {
        OutputFormat::Pretty
    }

    fn supported_formats(&self) -> &'static [OutputFormat] {
        PRETTY_TABLE_JSON
    }

    fn json_report(&self) -> &Self::JsonReport;

    fn jsonl_rows(&self) -> Option<&[Self::JsonlRow]> {
        None
    }

    fn render_pretty(&self) -> anyhow::Result<()>;
}

pub fn emit_collection<T: CollectionOutput>(
    global_format: Option<OutputFormat>,
    output: &T,
) -> anyhow::Result<()> {
    let format = formats::resolve(global_format, output.default_format());
    formats::ensure_supported(output.command_name(), format, output.supported_formats())?;
    match format {
        OutputFormat::Table => output.render_table(),
        OutputFormat::Json => formats::print_json(output.json_report()),
        OutputFormat::Csv => formats::print_csv_rows(output.csv_rows(), output.csv_headers()),
        OutputFormat::Pretty | OutputFormat::Jsonl => {
            unreachable!("collection outputs never support pretty/jsonl")
        }
    }
}

pub fn emit_detail<T: DetailOutput>(
    global_format: Option<OutputFormat>,
    output: &T,
) -> anyhow::Result<()> {
    let format = formats::resolve(global_format, output.default_format());
    formats::ensure_supported(output.command_name(), format, output.supported_formats())?;
    match format {
        OutputFormat::Table => output.render_human(),
        OutputFormat::Json => formats::print_json(output.json_report()),
        OutputFormat::Csv => unreachable!("detail outputs never support csv"),
        OutputFormat::Pretty | OutputFormat::Jsonl => {
            unreachable!("detail outputs never support pretty/jsonl")
        }
    }
}

pub fn emit_event<T: EventOutput>(
    global_format: Option<OutputFormat>,
    output: &T,
) -> anyhow::Result<()> {
    let format = formats::resolve(global_format, output.default_format());
    formats::ensure_supported(output.command_name(), format, output.supported_formats())?;
    match format {
        OutputFormat::Pretty | OutputFormat::Table => output.render_pretty(),
        OutputFormat::Json => formats::print_json(output.json_report()),
        OutputFormat::Jsonl => {
            let Some(rows) = output.jsonl_rows() else {
                anyhow::bail!(
                    "{} does not support `--format jsonl`",
                    output.command_name()
                );
            };
            formats::print_jsonl_rows(rows)
        }
        OutputFormat::Csv => unreachable!("event outputs never support csv"),
    }
}
