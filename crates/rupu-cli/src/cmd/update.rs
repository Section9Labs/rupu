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
    /// Deprecated no-op: `rupu update` no longer prompts, so there is
    /// nothing to skip. Still accepted so existing scripts and aliases
    /// that pass `-y` keep working.
    #[arg(long, short = 'y')]
    pub yes: bool,
    /// Override the configured channel for this run.
    #[arg(long, value_name = "beta|stable")]
    pub channel: Option<String>,
    /// Restore the previously-installed binary from the last backup.
    #[arg(long)]
    pub rollback: bool,
    /// Print this build's release-asset platform name (e.g. `linux-x64`)
    /// and exit. The release pipeline uses this to name assets from the
    /// binary itself, so the publisher and `rupu update` cannot drift.
    #[arg(long)]
    pub print_platform: bool,
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
    rupu_config::layer_files_locked(Some(&global_cfg_path), project_cfg_path.as_deref())
        .unwrap_or_default()
}

/// Whether `rupu update` must refuse, and what to say.
///
/// `--check` is deliberately always allowed: a user should be able to learn
/// they are behind no matter how they installed. Everything that would
/// *write* the binary is refused, because the manager owns the file — a
/// self-update would be silently reverted by the next `brew`/`apt`/`dnf`
/// upgrade (leaving the version to go backwards for no visible reason), or
/// refused outright by the read-only Nix store.
///
/// Takes the resolved owner rather than a bare bool so the message can name
/// the *right* manager: Homebrew and the AUR install the plain published
/// binary and carry no compile-time marker, so
/// [`crate::build_info::installed_owner`] falls back to the exe path.
///
/// Pure so the whole matrix is testable; the real owner is fixed at compile
/// time / by the exe path and cannot be varied from a test.
fn packaged_refusal(
    owner: Option<crate::build_info::InstallOwner>,
    pm: &str,
    check: bool,
    rollback: bool,
) -> Option<String> {
    let owner = owner?;
    if check && !rollback {
        return None;
    }
    let action = if rollback { "roll back" } else { "update" };
    let manager = match owner {
        crate::build_info::InstallOwner::Brew => "Homebrew",
        crate::build_info::InstallOwner::Nix => "Nix",
        crate::build_info::InstallOwner::Distro => "a system package",
    };
    // Only ever name a command we can spell correctly. `upgrade_command`
    // returns `None` for Nix (no single command) and for an unrecognized
    // distro — interpolating the `UNKNOWN_PACKAGE_MANAGER_HINT` prose into a
    // backticked command renders the uncopy-pasteable
    // `Run \`sudo your system package manager upgrade rupu\` instead.`
    let upgrade_hint = match crate::build_info::upgrade_command(owner, pm) {
        Some(cmd) => format!("Run `{cmd}` instead."),
        // Nix has no single upgrade command (profile vs flake vs NixOS
        // module); an unrecognized distro has no name we can spell.
        None if owner == crate::build_info::InstallOwner::Nix => {
            "Upgrade it through the Nix profile or flake that installed it instead.".to_string()
        }
        None => "Upgrade it with your system package manager instead.".to_string(),
    };
    Some(format!(
        "rupu was installed from {manager}, so it cannot {action} itself — \
         whatever it wrote would be overwritten (or refused) by the manager that owns the file.\n  \
         {upgrade_hint}\n  \
         `rupu update --check` still works and will tell you if a newer version exists."
    ))
}

pub async fn handle(args: UpdateArgs) -> ExitCode {
    match run(args).await {
        Ok(code) => code,
        Err(e) => crate::output::diag::fail(e),
    }
}

async fn run(args: UpdateArgs) -> anyhow::Result<ExitCode> {
    // Before config load and before any network call: this must work in a
    // bare build container with no credentials and no network. Output is
    // interpolated into an asset filename by the release pipeline, so it
    // is one clean line and nothing else.
    if args.print_platform {
        println!("{}", rupu_update::current_platform());
        return Ok(ExitCode::SUCCESS);
    }

    if let Some(message) = packaged_refusal(
        crate::build_info::installed_owner(),
        crate::build_info::package_manager_hint(),
        args.check,
        args.rollback,
    ) {
        anyhow::bail!(message);
    }

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
    // No confirmation prompt: running `rupu update` IS the confirmation.
    // Asking again buys nothing (the user typed the command), and it made
    // the command unusable anywhere stdin isn't a terminal — CI, a cron
    // tick, `rupu cp serve` — where `read_line` hit EOF and the run
    // silently reported "Aborted." Use `--check` to see whether an update
    // is available without installing it. The target version is not lost:
    // `UpdateProgress::start` below renders `from → to (channel)`.

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

#[cfg(test)]
mod packaged_tests {
    use super::*;
    use crate::build_info::InstallOwner;

    /// The common case under test: a distro-package install (.deb/.rpm/AUR).
    const DISTRO: Option<InstallOwner> = Some(InstallOwner::Distro);

    #[test]
    fn homebrew_is_refused_without_ever_saying_sudo_brew() {
        // Homebrew installs the plain published binary, so it carries no
        // compile-time package marker — the path is what gives it away. And
        // `brew` refuses to run under sudo, so naming it would be wrong.
        let msg = packaged_refusal(Some(InstallOwner::Brew), "apt", false, false)
            .expect("a brew-owned binary must refuse to self-update");
        assert!(msg.contains("Homebrew"), "message was: {msg}");
        assert!(
            msg.contains("Run `brew upgrade rupu` instead."),
            "message was: {msg}"
        );
        assert!(!msg.contains("sudo brew"), "message was: {msg}");
    }

    #[test]
    fn nix_is_refused_with_prose_not_a_made_up_command() {
        // The Nix store is read-only and there is no single upgrade command.
        let msg = packaged_refusal(Some(InstallOwner::Nix), "apt", false, false)
            .expect("a nix-store binary must refuse to self-update");
        assert!(msg.contains("Nix"), "message was: {msg}");
        // No invented upgrade command — the `--check` mention at the end is
        // the only backticked command, and that one is real.
        assert!(
            !msg.contains("Run `"),
            "must not name an upgrade command: {msg}"
        );
        assert!(msg.contains("Nix profile or flake"), "message was: {msg}");
    }

    #[test]
    fn unpackaged_installs_are_never_refused() {
        // Tarball / install.sh / dev builds own their own binary.
        assert!(packaged_refusal(None, "apt", false, false).is_none());
        assert!(packaged_refusal(None, "apt", true, false).is_none());
        assert!(packaged_refusal(None, "apt", false, true).is_none());
    }

    #[test]
    fn packaged_install_refuses_an_actual_update() {
        let msg = packaged_refusal(DISTRO, "apt", false, false).expect("must refuse");
        assert!(msg.contains("apt upgrade rupu"), "message was: {msg}");
        assert!(
            msg.contains("--check"),
            "must point at the still-working alternative"
        );
    }

    #[test]
    fn packaged_install_refuses_rollback() {
        // Rolling back under a package manager leaves it disagreeing with
        // what is on disk, and the next upgrade silently undoes it.
        let msg = packaged_refusal(DISTRO, "dnf", false, true).expect("must refuse");
        assert!(msg.contains("dnf"), "message was: {msg}");
    }

    #[test]
    fn packaged_install_still_allows_check() {
        // A user must be able to learn they are behind regardless of how
        // they installed.
        assert!(packaged_refusal(DISTRO, "apt", true, false).is_none());
    }

    #[test]
    fn the_message_names_the_package_manager_it_was_given() {
        let apt = packaged_refusal(DISTRO, "apt", false, false).unwrap();
        let dnf = packaged_refusal(DISTRO, "dnf", false, false).unwrap();
        assert!(apt.contains("apt") && !apt.contains("dnf"));
        assert!(dnf.contains("dnf") && !dnf.contains("apt"));
    }

    #[test]
    fn an_unknown_distro_still_refuses_without_naming_a_wrong_command() {
        let msg = packaged_refusal(DISTRO, "your system package manager", false, false)
            .expect("must still refuse");
        assert!(
            msg.contains("your system package manager"),
            "message was: {msg}"
        );
        // openSUSE-class fallback: must read as prose, NOT as a
        // copy-pasteable-looking (but non-functional) backticked command.
        assert!(
            msg.contains("Upgrade it with your system package manager instead."),
            "message was: {msg}"
        );
        assert!(
            !msg.contains("Run `sudo your system package manager upgrade rupu`"),
            "message was: {msg}"
        );
    }

    #[test]
    fn a_known_distro_keeps_the_exact_backticked_command() {
        let apt = packaged_refusal(DISTRO, "apt", false, false).expect("must refuse");
        assert!(
            apt.contains("Run `sudo apt upgrade rupu` instead."),
            "message was: {apt}"
        );
        let dnf = packaged_refusal(DISTRO, "dnf", false, false).expect("must refuse");
        assert!(
            dnf.contains("Run `sudo dnf upgrade rupu` instead."),
            "message was: {dnf}"
        );
    }

    #[test]
    fn check_does_not_excuse_rollback_on_a_packaged_install() {
        // `--check` alone is allowed, but pairing it with `--rollback` must
        // not smuggle a rollback past the refusal — the flags are not
        // mutually exclusive in clap, so this combination is reachable.
        let msg = packaged_refusal(DISTRO, "apt", true, true)
            .expect("check + rollback must still be refused");
        assert!(msg.contains("roll back"), "message was: {msg}");
    }
}
