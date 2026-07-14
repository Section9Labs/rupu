//! `rupu update` — download, verify, and install the latest release for
//! the configured (or overridden) channel.
//!
//! Thin dispatcher: all decision/download/verify/install logic lives in
//! `rupu-update`; this module only parses args, resolves config, and
//! prints. Exit code `10` from `--check` signals "an update is
//! available" to scripts without requiring stdout parsing.

use anyhow::Context;
use clap::Args;
use rupu_update::flow::{self, UpdateContext};
use rupu_update::{github, Channel, Decision};
use std::path::PathBuf;
use std::process::ExitCode;
use std::str::FromStr;

use crate::paths;

#[derive(Args, Debug)]
pub struct UpdateArgs {
    /// Only report whether an update is available; install nothing.
    #[arg(long)]
    pub check: bool,
    /// Reinstall even if already up to date (or downgrade to the
    /// channel's latest if this build is ahead of it).
    #[arg(long)]
    pub force: bool,
    /// Skip the confirmation prompt.
    #[arg(long, short = 'y')]
    pub yes: bool,
    /// Override the configured channel for this run.
    #[arg(long, value_name = "beta|stable")]
    pub channel: Option<String>,
    /// Restore the previously-installed binary from the last backup.
    #[arg(long)]
    pub rollback: bool,
}

/// Precedence: `--channel` flag > `[update].channel` config > "stable".
fn resolve_channel(flag: Option<&str>, cfg: Option<&str>) -> anyhow::Result<Channel> {
    let raw = flag.or(cfg).unwrap_or("stable");
    Channel::from_str(raw).map_err(|e| anyhow::anyhow!(e))
}

/// Load the layered global + project config the same way every other
/// subcommand does (see `cmd::webhook::load_cli_config` for the
/// original of this pattern). Exposed `pub(crate)` so the top-level
/// dispatcher (`lib.rs`) can reuse it for the passive update-notice gate.
pub(crate) fn load_cli_config() -> rupu_config::Config {
    let Ok(global_dir) = paths::global_dir() else {
        return rupu_config::Config::default();
    };
    let global_cfg_path = global_dir.join("config.toml");
    let pwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let project_root = paths::project_root_for(&pwd).ok().flatten();
    let project_cfg_path = project_root.as_ref().map(|p| p.join(".rupu/config.toml"));
    rupu_config::layer_files(Some(&global_cfg_path), project_cfg_path.as_deref())
        .unwrap_or_default()
}

pub async fn handle(args: UpdateArgs) -> ExitCode {
    match run(args).await {
        Ok(code) => code,
        Err(e) => crate::output::diag::fail(e),
    }
}

async fn run(args: UpdateArgs) -> anyhow::Result<ExitCode> {
    let cfg = load_cli_config();
    let channel = resolve_channel(args.channel.as_deref(), cfg.update.channel.as_deref())?;

    let exe = std::env::current_exe().context("resolve current exe")?;
    let exe = std::fs::canonicalize(&exe).unwrap_or(exe);
    let ctx = UpdateContext::from_env(crate::build_info::RELEASE_VERSION, channel, exe)?;

    if args.rollback {
        let target = ctx.exe_path.clone();
        let bak = rupu_update::install::backup_dir().join(format!(
            "rupu-{}",
            target
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("prev")
        ));
        let bytes =
            std::fs::read(&bak).with_context(|| format!("read backup at {}", bak.display()))?;
        rupu_update::install::swap_in_place(&bytes, &target, None)?;
        println!("Rolled back to {}", bak.display());
        return Ok(ExitCode::SUCCESS);
    }

    let src = github::GithubReleaseSource::new("Section9Labs/rupu");

    if args.check {
        let out = flow::check(&src, &ctx).await?;
        match out.decision {
            Decision::UpToDate => {
                println!("rupu {} ({channel}) is up to date.", ctx.current_version);
                return Ok(ExitCode::SUCCESS);
            }
            Decision::Update { to, .. } => {
                println!(
                    "Update available: {} → {to} ({channel}). Run 'rupu update'.",
                    ctx.current_version
                );
                return Ok(ExitCode::from(10));
            }
            Decision::Ahead => {
                println!(
                    "rupu {} is ahead of the {channel} channel.",
                    ctx.current_version
                );
                return Ok(ExitCode::SUCCESS);
            }
        }
    }

    if ctx.is_dev {
        anyhow::bail!(
            "this looks like a development build ({}); use `make install` / `cargo build` instead",
            ctx.exe_path.display()
        );
    }

    // Peek to confirm + print the target version before prompting.
    let out = flow::check(&src, &ctx).await?;
    match &out.decision {
        Decision::UpToDate if !args.force => {
            println!("Already up to date ({}).", ctx.current_version);
            return Ok(ExitCode::SUCCESS);
        }
        Decision::Ahead if !args.force => {
            println!(
                "rupu {} is ahead of the {channel} channel; nothing to do.",
                ctx.current_version
            );
            return Ok(ExitCode::SUCCESS);
        }
        _ => {}
    }
    if !args.yes {
        let to = out
            .latest
            .clone()
            .expect("check always sets latest when an asset exists");
        eprint!(
            "Update rupu {} → {to} ({channel})? [y/N] ",
            ctx.current_version
        );
        use std::io::Write;
        std::io::stderr().flush().ok();
        let mut line = String::new();
        std::io::stdin().read_line(&mut line).ok();
        if !matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
            println!("Aborted.");
            return Ok(ExitCode::SUCCESS);
        }
    }

    let dl = |url: String| {
        Box::pin(async move { github::download_bytes(&url).await })
            as std::pin::Pin<
                Box<
                    dyn std::future::Future<Output = Result<Vec<u8>, rupu_update::UpdateError>>
                        + Send,
                >,
            >
    };
    let apply = flow::DirectApply;
    let new = flow::install(&src, &ctx, args.force, &apply, dl).await?;
    println!("Updated rupu {} → {new} ({channel}).", ctx.current_version);
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_resolution_precedence() {
        assert_eq!(
            resolve_channel(Some("beta"), Some("stable")).unwrap(),
            Channel::Beta
        );
        assert_eq!(resolve_channel(None, Some("beta")).unwrap(), Channel::Beta);
        assert_eq!(resolve_channel(None, None).unwrap(), Channel::Stable);
        assert!(resolve_channel(Some("nightly"), None).is_err());
    }
}
