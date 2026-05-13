use crate::output::formats::{self, OutputFormat};
use serde::Serialize;

pub const TABLE_ONLY: &[OutputFormat] = &[OutputFormat::Table];
pub const TABLE_JSON: &[OutputFormat] = &[OutputFormat::Table, OutputFormat::Json];
pub const TABLE_JSON_CSV: &[OutputFormat] =
    &[OutputFormat::Table, OutputFormat::Json, OutputFormat::Csv];

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
    }
}
