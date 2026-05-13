use clap::ValueEnum;
use serde::Serialize;

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum, Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputFormat {
    Table,
    Pretty,
    Json,
    Jsonl,
    Csv,
}

impl OutputFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Table => "table",
            Self::Pretty => "pretty",
            Self::Json => "json",
            Self::Jsonl => "jsonl",
            Self::Csv => "csv",
        }
    }
}

impl std::fmt::Display for OutputFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

pub fn resolve(global: Option<OutputFormat>, default: OutputFormat) -> OutputFormat {
    global.unwrap_or(default)
}

pub fn ensure_supported(
    command_name: &str,
    format: OutputFormat,
    supported: &[OutputFormat],
) -> anyhow::Result<()> {
    if supported.contains(&format) {
        return Ok(());
    }
    let supported = supported
        .iter()
        .map(|value| format!("`{value}`"))
        .collect::<Vec<_>>()
        .join(", ");
    anyhow::bail!("{command_name} does not support `--format {format}` (supported: {supported})");
}

pub fn print_json<T: Serialize>(value: &T) -> anyhow::Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

pub fn print_csv_rows<T: Serialize>(
    rows: &[T],
    headers: Option<&'static [&'static str]>,
) -> anyhow::Result<()> {
    let stdout = std::io::stdout();
    let mut writer = csv::Writer::from_writer(stdout.lock());
    if rows.is_empty() {
        if let Some(headers) = headers {
            writer.write_record(headers)?;
        }
    }
    for row in rows {
        writer.serialize(row)?;
    }
    writer.flush()?;
    Ok(())
}

pub fn print_jsonl_rows<T: Serialize>(rows: &[T]) -> anyhow::Result<()> {
    for row in rows {
        println!("{}", serde_json::to_string(row)?);
    }
    Ok(())
}
