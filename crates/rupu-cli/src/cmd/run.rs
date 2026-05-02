//! `rupu run <agent> [prompt]`. Real impl lands in Task 4.

use std::process::ExitCode;

#[derive(clap::Args, Debug)]
pub struct Args {
    /// Name of the agent to run (matches an `agents/*.md` file).
    pub agent: String,
    /// Optional initial prompt; defaults to "go" if omitted.
    pub prompt: Option<String>,
}

pub async fn handle(_args: Args) -> ExitCode {
    eprintln!("not implemented yet");
    ExitCode::from(2)
}
