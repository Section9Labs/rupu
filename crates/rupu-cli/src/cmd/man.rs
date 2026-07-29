//! `rupu man` — render the man page to stdout.
//!
//! Generated from the live clap `Command`, so the page cannot describe
//! flags the binary does not have. The packaging job pipes this to
//! `rupu.1` and installs it; there is no checked-in man source to drift.

use clap::CommandFactory;
use std::io::Write;
use std::process::ExitCode;

pub fn handle() -> ExitCode {
    let mut buf: Vec<u8> = Vec::new();
    if let Err(e) = clap_mangen::Man::new(crate::Cli::command()).render(&mut buf) {
        eprintln!("rupu man: render failed: {e}");
        return ExitCode::from(1);
    }
    if let Err(e) = std::io::stdout().write_all(&buf) {
        eprintln!("rupu man: write failed: {e}");
        return ExitCode::from(1);
    }
    ExitCode::SUCCESS
}
