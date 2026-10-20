//! `rupu mcp serve` — deprecated. The MCP is served over HTTP by the
//! control plane now (`rupu cp serve`, endpoint `/mcp`); this command runs
//! `rupu cp serve --no-web --tools <its tools>` on the default bind.

use clap::{Args as ClapArgs, Subcommand, ValueEnum};
use std::process::ExitCode;

#[derive(Subcommand, Debug)]
pub enum Action {
    /// Deprecated: runs `rupu cp serve --no-web` (MCP over HTTP).
    Serve(ServeArgs),
}

#[derive(ClapArgs, Debug)]
pub struct ServeArgs {
    /// Ignored: stdio is no longer supported, the MCP is HTTP only.
    #[arg(long, value_enum)]
    pub transport: Option<TransportKind>,
    /// The tools to serve, in the agent `tools:` grammar: exact names,
    /// legacy aliases, `ns.*`, `core.*` or `*` (comma-separated or
    /// repeated). Passed to `rupu cp serve --tools`.
    #[arg(long, value_delimiter = ',')]
    pub tools: Vec<String>,
    /// Ignored: every tool is served and your MCP client's permissions
    /// decide what runs.
    #[arg(long)]
    pub mode: Option<String>,
}

#[derive(Copy, Clone, Debug, ValueEnum)]
pub enum TransportKind {
    Stdio,
    Http,
}

pub async fn handle(action: Action) -> ExitCode {
    match action {
        Action::Serve(args) => {
            eprintln!(
                "rupu mcp serve is deprecated and will be removed: it now runs `rupu cp serve --no-web` (MCP over HTTP at http://127.0.0.1:7878/mcp).\n\
                 Connect with: claude mcp add --transport http rupu http://127.0.0.1:7878/mcp"
            );
            if args.mode.is_some() {
                eprintln!(
                    "rupu mcp serve: ignoring --mode; every tool is served and your MCP client's permissions decide what runs"
                );
            }
            if args.transport.is_some() {
                eprintln!(
                    "rupu mcp serve: ignoring --transport; stdio is no longer supported, the MCP is HTTP only"
                );
            }
            crate::cmd::cp::serve_mcp_only(args.tools).await
        }
    }
}
