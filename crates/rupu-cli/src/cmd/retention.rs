/// [`rupu_runtime::retention::parse_retention_duration`], as an `anyhow`
/// error for the CLI's prune commands.
pub fn parse_retention_duration(value: &str) -> anyhow::Result<chrono::Duration> {
    Ok(rupu_runtime::retention::parse_retention_duration(value)?)
}
