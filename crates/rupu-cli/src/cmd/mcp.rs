//! `rupu mcp serve [--transport stdio|http] [--tools <grant>] [--mode <mode>]`
//! — an MCP server over the tool catalog for external clients (Claude
//! Desktop, Cursor, …). `rupu_mcp::CatalogServer` serves the grant; this
//! command builds what the tools run with.

use crate::paths;
use clap::{Args as ClapArgs, Subcommand, ValueEnum};
use rupu_mcp::{CatalogServer, StdioTransport, MCP_DEFAULT_GRANT};
use rupu_runtime::assembly::{AssemblyContext, WorkspaceBinding};
use rupu_scm::Registry;
use rupu_tools::{PermissionMode, RunIdentity, Surface};
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
    /// The tools to serve, in the agent `tools:` grammar: exact names,
    /// legacy aliases, `ns.*`, `core.*` or `*` (comma-separated or
    /// repeated). Default: the connector and findings tools
    /// (`scm.*,issues.*,github.*,gitlab.*,findings.*`).
    #[arg(long, value_delimiter = ',')]
    pub tools: Vec<String>,
    /// Permission mode each call is decided under: `bypass` (default — the
    /// MCP client's own confirmation UX is in front of every call),
    /// `readonly` (refuse workspace writes and external actions) or `ask`
    /// (no operator to prompt here, so it allows).
    #[arg(long, default_value = "bypass")]
    pub mode: String,
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
            Err(e) => crate::output::diag::fail(e),
        },
    }
}

async fn serve_inner(args: ServeArgs) -> anyhow::Result<()> {
    if matches!(args.transport, TransportKind::Http) {
        anyhow::bail!("http transport not wired in v0; use --transport stdio (the default)");
    }
    let mode = PermissionMode::parse(&args.mode)?;
    let global = paths::global_dir()?;
    paths::ensure_dir(&global)?;
    let pwd = std::env::current_dir()?;
    let project_root = paths::project_root_for(&pwd)?;
    let cfg_paths = paths::config_paths(&global, project_root.as_deref(), &pwd)?;
    let cfg = rupu_config::layer_files_locked(cfg_paths.layers())?;

    // `resolver_for`, not a bare `KeychainResolver::new()` — see
    // `crate::accounts`'s doc and `cmd/issues.rs`'s identical fix
    // (Ruling 7): a declared `[scm.gh-work]` account's SSO token needs
    // an `AccountSpec` to reach `get`'s near-expiry refresh branch.
    let resolver = crate::accounts::resolver_for(&cfg);
    // `rupu mcp serve` is a long-lived daemon that builds one registry at
    // startup and serves every future external MCP client request through
    // it (Claude Desktop, Cursor, ...) — there's no rupu run id to attach
    // to any of this traffic. No run exists yet at this point, so this SCM
    // traffic is not attributed to a ledger. A run-routing FlowSink would
    // close this; see the netflow per-run plan.
    let registry =
        Arc::new(Registry::discover(&resolver, &cfg, Arc::new(rupu_netflow::NullSink)).await);

    // What the served tools act on: the current directory's workspace, its
    // findings ledger under the `mcp` scope, attributed to this server
    // session (a minted run id, so `findings.verify` can tell it apart) —
    // a call-site context from the run assembler (W3/W4: no hand-built
    // tool contexts).
    let customer = cfg_paths.customer_slug.clone();
    let findings = crate::findings_opts::base_options(&global, &cfg.findings);
    let assembler = rupu_runtime::assembly::RunAssembler::new(AssemblyContext {
        project_root,
        config: cfg,
        customer,
        resolver: Arc::new(resolver),
        scm: Some(registry),
        findings,
        ..AssemblyContext::minimal(global)
    });
    let ctx = assembler.call_site_context(
        RunIdentity {
            run_id: format!("mcp_{}", ulid::Ulid::new()),
            surface: Surface::Agent,
            scope_name: Some("mcp".into()),
            ..Default::default()
        },
        &WorkspaceBinding {
            id: String::new(),
            path: pwd,
        },
        // A long-lived server is no run: a served `bash`'s flows have no run
        // ledger to land in (`rupu_runtime::netflow`).
        Arc::new(rupu_netflow::NullSink),
    );
    let tools = (!args.tools.is_empty()).then_some(args.tools.as_slice());
    let (server, unavailable) = CatalogServer::new(StdioTransport::new(), ctx, mode, tools)?;
    // stdout is the JSON-RPC channel: every notice goes to stderr.
    for u in &unavailable {
        eprintln!(
            "{}: {}",
            rupu_tools::grant::UNAVAILABLE_NOTICE_KIND,
            u.notice_message()
        );
    }
    tracing::info!(
        tools = ?server.tool_names(),
        grant = ?tools.map_or_else(
            || MCP_DEFAULT_GRANT.iter().map(|s| s.to_string()).collect(),
            <[String]>::to_vec,
        ),
        "mcp serve"
    );
    server
        .run()
        .await
        .map_err(|e| anyhow::anyhow!("mcp server: {e}"))
}
