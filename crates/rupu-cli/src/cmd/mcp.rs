//! `rupu mcp serve [--transport stdio|http]` — JSON-RPC MCP server
//! for external clients (Claude Desktop, Cursor, etc).

use crate::paths;
use clap::{Args as ClapArgs, Subcommand, ValueEnum};
use rupu_mcp::{McpPermission, McpServer, StdioTransport};
use rupu_scm::Registry;
use std::process::ExitCode;
use std::sync::Arc;

#[derive(Subcommand, Debug)]
pub enum Action {
    /// Run the MCP server for an external MCP-aware client.
    Serve(ServeArgs),
}

#[derive(ClapArgs, Debug)]
pub struct ServeArgs {
    /// Transport. v0 ships stdio only; http returns NotWiredInV0.
    #[arg(long, value_enum, default_value_t = TransportKind::Stdio)]
    pub transport: TransportKind,
}

#[derive(Copy, Clone, Debug, ValueEnum)]
pub enum TransportKind {
    Stdio,
    Http,
}

pub async fn handle(action: Action) -> ExitCode {
    match action {
        Action::Serve(args) => match serve_inner(args).await {
            Ok(()) => ExitCode::from(0),
            Err(e) => {
                eprintln!("rupu mcp serve: {e}");
                ExitCode::from(1)
            }
        },
    }
}

async fn serve_inner(args: ServeArgs) -> anyhow::Result<()> {
    if matches!(args.transport, TransportKind::Http) {
        anyhow::bail!("http transport not wired in v0; use --transport stdio (the default)");
    }
    let global = paths::global_dir()?;
    paths::ensure_dir(&global)?;
    let pwd = std::env::current_dir()?;
    let project_root = paths::project_root_for(&pwd)?;
    let global_cfg = global.join("config.toml");
    let project_cfg = project_root.as_ref().map(|p| p.join(".rupu/config.toml"));
    let cfg = rupu_config::layer_files(Some(&global_cfg), project_cfg.as_deref())?;

    let resolver = rupu_auth::KeychainResolver::new();
    let registry = Arc::new(Registry::discover(&resolver, &cfg).await);

    // External-client mode: trust the upstream client's permission UX.
    // Bypass mode + allow-all listing — Claude Desktop / Cursor handle
    // confirmation prompts themselves.
    let permission = McpPermission::allow_all();
    let server = McpServer::new(registry, StdioTransport::new(), permission);
    server
        .run()
        .await
        .map_err(|e| anyhow::anyhow!("mcp server: {e}"))
}
