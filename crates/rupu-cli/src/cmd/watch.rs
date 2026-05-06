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
    match handle_inner(args) {
        Ok(()) => ExitCode::from(0),
        Err(e) => {
            eprintln!("rupu watch: {e}");
            ExitCode::from(1)
        }
    }
}

fn handle_inner(args: WatchArgs) -> anyhow::Result<()> {
    let runs_dir = paths::global_dir()?.join("runs");
    if args.replay {
        let pace_us = (1_000_000.0 / args.pace.max(0.1)) as u64;
        rupu_tui::run_replay(args.run_id, runs_dir, pace_us)
            .map_err(|e| anyhow::anyhow!("tui: {e}"))
    } else {
        rupu_tui::run_watch(args.run_id, runs_dir).map_err(|e| anyhow::anyhow!("tui: {e}"))
    }
}
