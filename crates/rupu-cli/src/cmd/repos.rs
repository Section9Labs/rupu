//! `rupu repos list [--platform <name>]` — list configured-platform repos.

use crate::paths;
use clap::{Args as ClapArgs, Subcommand};
use comfy_table::{ContentArrangement, Table};
use rupu_scm::{Platform, Registry};
use std::process::ExitCode;
use std::sync::Arc;

#[derive(Subcommand, Debug)]
pub enum Action {
    /// List repositories accessible via configured SCM platforms.
    List(ListArgs),
}

#[derive(ClapArgs, Debug)]
pub struct ListArgs {
    /// Filter to one platform (`github` | `gitlab`). Default: all.
    #[arg(long)]
    pub platform: Option<String>,
}

pub async fn handle(action: Action) -> ExitCode {
    match action {
        Action::List(args) => match list_inner(args).await {
            Ok(()) => ExitCode::from(0),
            Err(e) => {
                eprintln!("rupu repos list: {e}");
                ExitCode::from(1)
            }
        },
    }
}

async fn list_inner(args: ListArgs) -> anyhow::Result<()> {
    let global = paths::global_dir()?;
    paths::ensure_dir(&global)?;
    let pwd = std::env::current_dir()?;
    let project_root = paths::project_root_for(&pwd)?;
    let global_cfg = global.join("config.toml");
    let project_cfg = project_root.as_ref().map(|p| p.join(".rupu/config.toml"));
    let cfg = rupu_config::layer_files(Some(&global_cfg), project_cfg.as_deref())?;

    let resolver = rupu_auth::KeychainResolver::new();
    let registry = Arc::new(Registry::discover(&resolver, &cfg).await);

    let platforms: Vec<Platform> = match args.platform.as_deref() {
        Some(s) => vec![s.parse().map_err(|e: String| anyhow::anyhow!(e))?],
        None => vec![Platform::Github, Platform::Gitlab],
    };

    let mut table = Table::new();
    table.set_content_arrangement(ContentArrangement::Dynamic);
    table.set_header(vec![
        "Platform",
        "Owner/Repo",
        "Default branch",
        "Visibility",
    ]);

    let mut any_listed = false;
    let mut any_skipped = false;
    for p in platforms {
        let Some(conn) = registry.repo(p) else {
            eprintln!("(skipped {p}: no credential — run `rupu auth login --provider {p}`)");
            any_skipped = true;
            continue;
        };
        let repos = conn.list_repos().await?;
        for r in repos {
            table.add_row(vec![
                p.to_string(),
                format!("{}/{}", r.r.owner, r.r.repo),
                r.default_branch,
                if r.private {
                    "private".into()
                } else {
                    "public".into()
                },
            ]);
            any_listed = true;
        }
    }
    if !any_listed {
        if !any_skipped {
            eprintln!("No repos to list across configured platforms.");
        }
        return Ok(());
    }
    println!("{table}");
    Ok(())
}
