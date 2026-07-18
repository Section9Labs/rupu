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
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::str::FromStr;

use crate::cmd::apply_update;
use crate::cmd::ui::UiPrefs;
use crate::cmd::update_progress;
use crate::paths;
use indicatif::ProgressBar;

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
        apply_maybe_elevated(&bytes, &target)?;
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
    let to = out
        .latest
        .clone()
        .expect("check always sets latest when an asset exists");
    if !args.yes {
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

    // Resolve UI prefs so the progress bar matches the configured theme.
    // `resolve` also installs the active palette as a side effect — this
    // command never otherwise initializes it (see `output::palette`).
    let prefs = UiPrefs::resolve(&cfg.ui, false, None, None, None);
    let progress = update_progress::UpdateProgress::start(
        &ctx.current_version.to_string(),
        &to.to_string(),
        channel,
        &prefs,
    );
    let dl_bar = progress.bar();
    let themed = progress.themed();

    let dl = move |url: String| {
        let bar = dl_bar.clone();
        Box::pin(async move {
            // The `.sha256` sidecar is a few dozen bytes — download it
            // quietly; the bar is for the binary only.
            if url.ends_with(".sha256") {
                return github::download_bytes(&url).await;
            }
            match bar {
                Some(bar) => {
                    // Configure the bar from the first progress tick, which
                    // carries the authoritative length: determinate when the
                    // server sent Content-Length, else a bytes-only spinner
                    // (so it never renders as a stuck 0%).
                    let mut configured = false;
                    let bytes = github::download_bytes_with_progress(&url, |done, total| {
                        if !configured {
                            configured = true;
                            match total {
                                Some(total) => bar.set_length(total),
                                None => {
                                    update_progress::switch_to_indeterminate_download(&bar, themed)
                                }
                            }
                        }
                        bar.set_position(done);
                    })
                    .await?;
                    // Bytes are in; verify/sign/swap is not byte-measurable,
                    // so the bar morphs into an indeterminate spinner.
                    update_progress::switch_to_installing(&bar, themed);
                    Ok(bytes)
                }
                None => github::download_bytes(&url).await,
            }
        })
            as std::pin::Pin<
                Box<
                    dyn std::future::Future<Output = Result<Vec<u8>, rupu_update::UpdateError>>
                        + Send,
                >,
            >
    };
    let apply = ElevatingApply { pb: progress.bar() };
    let check = rupu_update::CodesignCheck;
    let new = match flow::install(&src, &ctx, args.force, &apply, &check, dl).await {
        Ok(new) => new,
        Err(e) => {
            // Clear the live bar/spinner before the error surfaces on stderr.
            progress.abandon();
            return Err(e.into());
        }
    };
    progress.finish(&new.to_string(), channel);
    Ok(ExitCode::SUCCESS)
}

// ---------------------------------------------------------------------------
// Elevation: swap in place directly when the install directory is
// user-writable; otherwise stage the verified bytes and re-exec ourself
// under `sudo` as `__apply-update`, which re-verifies the checksum before
// the privileged swap.
// ---------------------------------------------------------------------------

pub struct ElevatingApply {
    /// The live progress bar, if any — suspended around the `sudo` prompt
    /// so the steady-tick spinner doesn't fight the password entry.
    pub pb: Option<ProgressBar>,
}

impl flow::ApplyStrategy for ElevatingApply {
    fn apply(&self, verified: &[u8], target: &Path) -> Result<(), rupu_update::UpdateError> {
        let dir = target.parent().ok_or_else(|| {
            rupu_update::UpdateError::Install("target has no parent directory".into())
        })?;
        if apply_update::dir_writable(dir) {
            return flow::DirectApply.apply(verified, target);
        }

        // Stage the verified bytes outside the (unwritable) target dir,
        // then re-exec ourself under sudo to do the actual swap.
        let cache = rupu_update::install::backup_dir()
            .parent()
            .map(|p| p.join("cache").join("update"))
            .ok_or_else(|| {
                rupu_update::UpdateError::Install("could not derive cache dir".into())
            })?;
        std::fs::create_dir_all(&cache).map_err(rupu_update::UpdateError::Io)?;
        let staged = cache.join("rupu.staged");
        std::fs::write(&staged, verified).map_err(rupu_update::UpdateError::Io)?;
        let sha = rupu_update::verify::sha256_hex(verified);
        let self_exe = std::env::current_exe().map_err(rupu_update::UpdateError::Io)?;

        // Resolve the backup path in the USER (parent, unprivileged)
        // context — same convention `DirectApply` uses — and pass it
        // explicitly so the privileged step doesn't recompute
        // `backup_dir()` under `sudo`'s (possibly root) `$HOME`. Without
        // this, `rupu update --rollback` (run as the user afterward)
        // looks in the user's `~/.rupu/backups` and never finds a
        // backup the privileged step wrote under root's home.
        let backup = rupu_update::install::backup_dir().join(format!(
            "rupu-{}",
            target
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("prev")
        ));

        // Run the elevation prompt + swap with the progress bar suspended,
        // so `sudo`'s password prompt owns the terminal cleanly.
        let run = || {
            eprintln!("Elevating to install into {} …", dir.display());
            std::process::Command::new("sudo")
                .arg(&self_exe)
                .arg("__apply-update")
                .arg("--from")
                .arg(&staged)
                .arg("--to")
                .arg(target)
                .arg("--sha256")
                .arg(&sha)
                .arg("--backup")
                .arg(&backup)
                .status()
        };
        let status = match &self.pb {
            Some(pb) => pb.suspend(run),
            None => run(),
        }
        .map_err(|e| rupu_update::UpdateError::Install(format!("sudo failed to start: {e}")))?;
        if !status.success() {
            return Err(rupu_update::UpdateError::Install(format!(
                "privileged apply failed; run manually: sudo {} __apply-update --from {} --to {} --sha256 {} --backup {}",
                self_exe.display(),
                staged.display(),
                target.display(),
                sha,
                backup.display()
            )));
        }
        Ok(())
    }
}

/// Used by `--rollback`, which reuses the same elevation decision as a
/// normal install (a rollback is just "swap this file back in").
fn apply_maybe_elevated(bytes: &[u8], target: &Path) -> anyhow::Result<()> {
    use flow::ApplyStrategy as _;
    ElevatingApply { pb: None }
        .apply(bytes, target)
        .map_err(|e| anyhow::anyhow!(e))
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
