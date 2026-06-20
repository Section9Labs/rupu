//! `rupu cp` — control-plane HTTP server subcommand.

use crate::paths;
use clap::Subcommand;
use std::net::SocketAddr;
use std::process::ExitCode;

#[derive(Subcommand, Debug)]
pub enum Action {
    /// Start the control-plane HTTP server.
    Serve {
        /// Address to bind. Defaults to 127.0.0.1:7878.
        #[arg(long, default_value = "127.0.0.1:7878")]
        bind: SocketAddr,
        /// Optional bearer token. If set, `/api/*` requires
        /// `Authorization: Bearer <token>` (the web UI and `/healthz` remain
        /// open on localhost).
        #[arg(long)]
        token: Option<String>,
        /// Do not open the served URL in a browser on startup. By default the
        /// URL is opened when running interactively (a terminal); the URL is
        /// always printed regardless.
        #[arg(long)]
        no_open: bool,
    },
}

pub async fn handle(action: Action) -> ExitCode {
    let result = match action {
        Action::Serve {
            bind,
            token,
            no_open,
        } => {
            let global_dir = match paths::global_dir() {
                Ok(d) => d,
                Err(e) => {
                    eprintln!("error: {e:#}");
                    return ExitCode::FAILURE;
                }
            };
            rupu_cp::serve(rupu_cp::ServeOpts {
                bind,
                token,
                global_dir,
                open_browser: !no_open,
            })
            .await
        }
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}
