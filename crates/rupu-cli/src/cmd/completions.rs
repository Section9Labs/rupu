//! `rupu completions <shell>` — print a shell-completion bootstrap;
//! `rupu completions install` — write it to the canonical path.
//!
//! Two flavors:
//!
//! 1. **Dynamic bootstrap (default).** A short script that calls
//!    back into `rupu` at completion time. This is the only mode
//!    that supports completion of agent / workflow names from disk
//!    (wired via `ArgValueCompleter` on the relevant positionals).
//!    Implemented by re-invoking ourselves with `COMPLETE=<shell>`
//!    set, which `lib.rs::run` -> `CompleteEnv::complete()` recognizes
//!    and prints the registration snippet for.
//!
//! 2. **Static script (`--static`).** Output of `clap_complete::generate`
//!    — a self-contained file that handles flag + subcommand
//!    completion but cannot do dynamic value completion. Useful for
//!    distribution channels that ship a frozen completion file.

use anyhow::{anyhow, Context, Result};
use clap::CommandFactory;
use clap::{Args as ClapArgs, Subcommand};
use clap_complete::Shell;
use std::fs;
use std::path::PathBuf;
use std::process::{Command, ExitCode, Stdio};

use crate::Cli;

#[derive(Subcommand, Debug)]
pub enum Action {
    /// Print a completion script for the given shell to stdout.
    Print(PrintArgs),
    /// Write the completion script to the canonical location for
    /// the given shell. If `--shell` is omitted, auto-detect from
    /// `$SHELL` (best-effort).
    Install(InstallArgs),
}

#[derive(ClapArgs, Debug)]
pub struct PrintArgs {
    /// Target shell.
    pub shell: Shell,
    /// Emit a static, self-contained script (no runtime callbacks).
    /// Drops dynamic agent / workflow name completion in exchange
    /// for offline-installable output. Default: dynamic bootstrap.
    #[arg(long, default_value_t = false)]
    pub r#static: bool,
}

#[derive(ClapArgs, Debug)]
pub struct InstallArgs {
    /// Target shell. Auto-detected from `$SHELL` when omitted.
    #[arg(long)]
    pub shell: Option<Shell>,
    /// Install path override. Defaults to the conventional location
    /// for the target shell (printed at the end of the run).
    #[arg(long)]
    pub path: Option<PathBuf>,
    /// Install the static script (see `print --static`). Default:
    /// dynamic bootstrap.
    #[arg(long, default_value_t = false)]
    pub r#static: bool,
}

pub async fn handle(action: Action) -> ExitCode {
    let result = match action {
        Action::Print(a) => print(a),
        Action::Install(a) => install(a),
    };
    match result {
        Ok(()) => ExitCode::from(0),
        Err(e) => {
            eprintln!("rupu completions: {e:#}");
            ExitCode::from(1)
        }
    }
}

fn print(args: PrintArgs) -> Result<()> {
    if args.r#static {
        let mut cmd = Cli::command();
        clap_complete::generate(args.shell, &mut cmd, "rupu", &mut std::io::stdout());
        Ok(())
    } else {
        let bootstrap = generate_bootstrap(args.shell)?;
        std::io::Write::write_all(&mut std::io::stdout(), bootstrap.as_bytes())?;
        Ok(())
    }
}

fn install(args: InstallArgs) -> Result<()> {
    let shell = match args.shell {
        Some(s) => s,
        None => detect_shell()?,
    };
    let path = match args.path {
        Some(p) => p,
        None => default_install_path(shell)?,
    };

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create parent dir {}", parent.display()))?;
    }

    let body = if args.r#static {
        let mut buf: Vec<u8> = Vec::new();
        let mut cmd = Cli::command();
        clap_complete::generate(shell, &mut cmd, "rupu", &mut buf);
        buf
    } else {
        generate_bootstrap(shell)?.into_bytes()
    };

    fs::write(&path, &body).with_context(|| format!("write {}", path.display()))?;

    let mode = if args.r#static { "static" } else { "dynamic" };
    println!(
        "✓ wrote {} {} completion to {}",
        shell,
        mode,
        path.display()
    );
    if let Some(hint) = post_install_hint(shell, args.r#static) {
        println!();
        println!("{hint}");
    }
    Ok(())
}

/// Re-invokes the running binary with `COMPLETE=<shell>` set; that
/// triggers `CompleteEnv::complete()` in `lib.rs::run` which prints
/// the registration snippet and exits 0.
///
/// **zsh special-case.** The default snippet from `clap_complete`
/// surfaces flag candidates (`--input`, `--mode`, …) alongside
/// positional value candidates at any cursor position. That's
/// technically correct (flags are valid anywhere on the line) but
/// reads as noise when the user is plainly mid-typing a workflow /
/// agent name. We override the zsh snippet with a hand-rolled one
/// that splits candidates into "starts with `-` = flag" vs. "value"
/// buckets and only shows the bucket matching what the user is
/// actually typing. Empty value bucket emits a `_message` hint
/// pointing at `rupu init --with-samples` so users on a fresh shell
/// see something instead of silence.
fn generate_bootstrap(shell: Shell) -> Result<String> {
    if matches!(shell, Shell::Zsh) {
        return generate_zsh_bootstrap();
    }
    let exe = std::env::current_exe().context("locate current executable")?;
    let shell_name = shell_var_value(shell)?;
    let output = Command::new(&exe)
        .env("COMPLETE", shell_name)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .with_context(|| format!("invoke {} with COMPLETE={shell_name}", exe.display()))?;
    if !output.status.success() {
        return Err(anyhow!(
            "rupu (COMPLETE={shell_name}) exited {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    String::from_utf8(output.stdout).context("non-utf8 bootstrap output")
}

/// Hand-rolled zsh dynamic-completion bootstrap. Implements the same
/// `COMPLETE=zsh` env-var protocol as `clap_complete::CompleteEnv`,
/// then post-processes the candidate list before handing it to
/// `_describe` / `_message`.
fn generate_zsh_bootstrap() -> Result<String> {
    let exe = std::env::current_exe().context("locate current executable")?;
    let exe_str = exe.to_string_lossy();
    Ok(format!(
        r##"#compdef rupu
# Hand-rolled zsh completion for rupu. See
# `crates/rupu-cli/src/cmd/completions.rs::generate_zsh_bootstrap`
# for the rationale (flag/value bucket split + empty-candidate hint).
_rupu() {{
    local _CLAP_COMPLETE_INDEX=$(expr $CURRENT - 1)
    local _CLAP_IFS=$'\n'
    local current_word="${{words[$CURRENT]}}"

    local raw=("${{(@f)$( \
        _CLAP_IFS="$_CLAP_IFS" \
        _CLAP_COMPLETE_INDEX="$_CLAP_COMPLETE_INDEX" \
        COMPLETE="zsh" \
        {exe_str} -- "${{words[@]}}" 2>/dev/null \
    )}}")

    local -a values=()
    local -a flags=()
    local entry
    for entry in $raw; do
        local val="${{entry%%:*}}"
        # Treat lines whose value-segment starts with `-` as flags
        # (covers `--long`, `-s`, `--long=value`). Everything else is
        # a positional value candidate.
        if [[ "$val" == -* ]]; then
            flags+=("$entry")
        else
            values+=("$entry")
        fi
    done

    if [[ "$current_word" == -* ]]; then
        # User is typing a flag — show only flag candidates.
        [[ -n $flags ]] && _describe 'option' flags
    else
        if [[ -n $values ]]; then
            _describe 'value' values
        elif [[ -n $flags ]]; then
            # No positional candidates available. Tell the user why
            # before falling through to flags so the cursor isn't left
            # with a silent "no completion." `_message` lines are
            # informational and don't complete to text.
            _message $'no agents/workflows found in cwd or ~/.rupu — try `rupu init --with-samples`'
            _describe 'option' flags
        else
            _message $'no candidates'
        fi
    fi
}}

compdef _rupu rupu
"##
    ))
}

fn shell_var_value(shell: Shell) -> Result<&'static str> {
    Ok(match shell {
        Shell::Bash => "bash",
        Shell::Zsh => "zsh",
        Shell::Fish => "fish",
        Shell::PowerShell => "powershell",
        Shell::Elvish => "elvish",
        other => return Err(anyhow!("unsupported shell for dynamic mode: {other}")),
    })
}

fn detect_shell() -> Result<Shell> {
    // `$SHELL` carries the absolute path to the user's login shell on
    // POSIX systems. We only care about the basename.
    let shell_path =
        std::env::var("SHELL").map_err(|_| anyhow!("$SHELL not set; pass --shell explicitly"))?;
    let basename = std::path::Path::new(&shell_path)
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| anyhow!("could not parse $SHELL: {shell_path}"))?;
    match basename {
        "bash" => Ok(Shell::Bash),
        "zsh" => Ok(Shell::Zsh),
        "fish" => Ok(Shell::Fish),
        "pwsh" | "powershell" => Ok(Shell::PowerShell),
        other => Err(anyhow!(
            "unrecognized shell `{other}`; pass --shell explicitly"
        )),
    }
}

fn default_install_path(shell: Shell) -> Result<PathBuf> {
    let home = dirs::home_dir().ok_or_else(|| anyhow!("could not resolve $HOME"))?;
    let path = match shell {
        Shell::Bash => home
            .join(".local")
            .join("share")
            .join("bash-completion")
            .join("completions")
            .join("rupu"),
        Shell::Zsh => {
            // Honor $ZDOTDIR if set; fall back to $HOME.
            let zsh_root = std::env::var_os("ZDOTDIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| home.clone());
            zsh_root.join(".zfunc").join("_rupu")
        }
        Shell::Fish => home
            .join(".config")
            .join("fish")
            .join("completions")
            .join("rupu.fish"),
        Shell::PowerShell => home
            .join("Documents")
            .join("PowerShell")
            .join("Completions")
            .join("rupu.ps1"),
        Shell::Elvish => home.join(".config").join("elvish").join("rupu.elv"),
        _ => return Err(anyhow!("unsupported shell: {shell}")),
    };
    Ok(path)
}

fn post_install_hint(shell: Shell, is_static: bool) -> Option<String> {
    let dynamic_note = if is_static {
        ""
    } else {
        "Dynamic mode requires `rupu` to remain on $PATH at completion time.\n"
    };
    let body = match shell {
        Shell::Zsh => Some(format!(
            "{dynamic_note}\
             Add the install dir to your fpath if it isn't already. In ~/.zshrc:\n  \
             fpath=(\"$HOME/.zfunc\" $fpath)\n  \
             autoload -Uz compinit && compinit\n\
             Then restart your shell."
        )),
        Shell::Bash => Some(format!(
            "{dynamic_note}\
             Make sure bash-completion is installed and that the completions\n\
             dir is sourced. On most systems the file above is auto-loaded;\n\
             otherwise add to ~/.bashrc:\n  \
             source ~/.local/share/bash-completion/completions/rupu"
        )),
        Shell::Fish => Some(format!(
            "{dynamic_note}\
             Restart your shell or run `exec fish` to pick up the new completions."
        )),
        Shell::PowerShell => Some(format!(
            "{dynamic_note}\
             Add to your PowerShell profile:\n  \
             . \"$HOME/Documents/PowerShell/Completions/rupu.ps1\""
        )),
        _ => None,
    };
    body
}
