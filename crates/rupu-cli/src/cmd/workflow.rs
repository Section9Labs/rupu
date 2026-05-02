//! `rupu workflow` subcommand. Real impl lands in Task 7.

use clap::Subcommand;
use std::process::ExitCode;

#[derive(Subcommand, Debug)]
pub enum Action {
    /// Placeholder; real subcommands land in Task 7.
    #[command(hide = true)]
    Stub,
}

pub async fn handle(_action: Action) -> ExitCode {
    eprintln!("not implemented yet");
    ExitCode::from(2)
}
