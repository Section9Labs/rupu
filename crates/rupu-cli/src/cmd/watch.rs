use crate::paths;
use clap::Args;
use std::process::ExitCode;

#[derive(Debug, Args)]
pub struct WatchArgs {
    /// Run id (e.g. `run_01HZ…`)
    pub run_id: String,

    /// Replay a finished run instead of tailing live.
    #[arg(long)]
    pub replay: bool,

    /// Replay pace in events per second (only with --replay).
    #[arg(long, default_value_t = 10.0)]
    pub pace: f32,
}

pub async fn handle(args: WatchArgs) -> ExitCode {
    handle_inner(args)
}

fn handle_inner(args: WatchArgs) -> ExitCode {
    let runs_dir = match paths::global_dir() {
        Ok(d) => d.join("runs"),
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::from(1);
        }
    };
    let result = if args.replay {
        let pace_us = (1_000_000.0 / args.pace.max(0.1)) as u64;
        rupu_tui::run_replay(args.run_id, runs_dir, pace_us)
    } else {
        rupu_tui::run_watch(args.run_id, runs_dir)
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(rupu_tui::TuiError::RunNotFound(id, dir)) => {
            eprintln!(
                "error: run \"{id}\" not found in {}/. Suggest `rupu workflow runs`",
                dir.display()
            );
            ExitCode::from(2)
        }
        Err(e) => {
            eprintln!("rupu watch: tui: {e}");
            ExitCode::from(1)
        }
    }
}
