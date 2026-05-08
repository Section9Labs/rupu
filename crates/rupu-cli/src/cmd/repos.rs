//! `rupu repos list [--platform <name>]` — list configured-platform repos.

use crate::paths;
use clap::{Args as ClapArgs, Subcommand};
use comfy_table::Cell;
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
    /// Disable colored output (also honored: `NO_COLOR` env,
    /// `[ui].color = "never"` in config).
    #[arg(long)]
    pub no_color: bool,
}

pub async fn handle(action: Action) -> ExitCode {
    match action {
        Action::List(args) => match list_inner(args).await {
            Ok(()) => ExitCode::from(0),
            Err(e) => crate::output::diag::fail(e),
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

    let prefs = crate::cmd::ui::UiPrefs::resolve(&cfg.ui, args.no_color, None, None);

    let mut table = crate::output::tables::new_table();
    table.set_header(vec![
        "Platform",
        "Owner/Repo",
        "Default branch",
        "Visibility",
    ]);

    let mut any_listed = false;
    let mut any_skipped = false;
    let mut any_private = false;
    for p in platforms {
        let Some(conn) = registry.repo(p) else {
            crate::output::diag::skip(
                &prefs,
                p.to_string(),
                "no credential",
                format!("rupu auth login --provider {p}"),
            );
            any_skipped = true;
            continue;
        };
        let repos = conn.list_repos().await?;
        for r in repos {
            if r.private {
                any_private = true;
            }
            // Visibility coloring: private → dim (slate), public → green
            // (mirrors GitHub's "open" green for the not-locked case).
            // When no_color is set, both render plain.
            let visibility_cell = if !prefs.use_color() {
                Cell::new(if r.private { "private" } else { "public" })
            } else if r.private {
                Cell::new("private").fg(comfy_table::Color::DarkGrey)
            } else {
                Cell::new("public").fg(crate::output::tables::status_color("open", &prefs)
                    .unwrap_or(comfy_table::Color::Reset))
            };
            table.add_row(vec![
                Cell::new(p.to_string()),
                Cell::new(format!("{}/{}", r.r.owner, r.r.repo)),
                Cell::new(&r.default_branch),
                visibility_cell,
            ]);
            any_listed = true;
        }
    }
    if !any_listed {
        if !any_skipped {
            println!("No repos to list across configured platforms.");
        }
        return Ok(());
    }
    println!("{table}");

    // GitHub-specific scope diagnostic: if no private repos came back
    // AND the user expected some, the most common cause is that the
    // stored token doesn't carry the `repo` scope. Probe the GitHub
    // token's scopes via `GET /user`'s `X-OAuth-Scopes` header and
    // emit an actionable warn when the scope is missing. Skipped
    // entirely when GitHub credentials aren't configured (no
    // `github_extras`) or the probe call fails (unknown scope set —
    // we'd rather say nothing than yell).
    if !any_private {
        if let Some(extras) = registry.github_extras() {
            if let Some(scopes) = extras.fetch_token_scopes().await {
                emit_private_repo_diag(&prefs, &scopes);
            }
        }
    }

    Ok(())
}

/// Three cases to handle when private repos are absent:
///
/// 1. **Empty `X-OAuth-Scopes` header.** The token is a GitHub App
///    user-to-server token, not a classic OAuth token. GitHub Apps
///    don't grant OAuth scopes; access is per-installation. (rupu's
///    SSO flow uses the GitHub Copilot client_id `Iv1.…` — see
///    `crates/rupu-auth/src/oauth/providers.rs`.) Re-logging in via
///    `--mode sso` won't change anything because no scope is involved.
///    The fix is either an installation-level grant on the user's
///    org, or switching to `--mode api-key` with a classic PAT
///    that has the `repo` scope.
///
/// 2. **Has scopes but `repo` is missing.** Classic OAuth token with
///    insufficient scope. Re-login (or PAT) with the right scope.
///
/// 3. **Has `repo`.** Token is fully privileged; the repos must
///    genuinely not exist or the user lacks access. Say nothing.
fn emit_private_repo_diag(prefs: &crate::cmd::ui::UiPrefs, scopes: &[String]) {
    if scopes.is_empty() {
        // Case 1: GitHub App user token.
        crate::output::diag::warn_with_hint(
            prefs,
            "no private github repos shown — your stored token is a GitHub App \
             user-to-server token (rupu impersonates the GitHub Copilot client). \
             GitHub App tokens don't carry OAuth scopes; they grant per-installation \
             access, so private repos only appear from orgs / accounts where the \
             Copilot app is installed and has access to those repos.",
            "use `rupu auth login --provider github --mode api-key` with a classic \
             PAT (https://github.com/settings/tokens) that has the `repo` scope, \
             OR install the Copilot app on the relevant org / repos at \
             https://github.com/settings/installations.",
        );
        return;
    }
    let has_repo = scopes.iter().any(|s| s == "repo");
    if has_repo {
        // Case 3: token IS privileged. Stay quiet — no private repos
        // visible just means the user genuinely has none accessible.
        return;
    }
    // Case 2: classic OAuth token, but `repo` scope missing.
    let has_public_only = scopes.iter().any(|s| s == "public_repo");
    let detail = if has_public_only {
        "your stored github token only has the `public_repo` scope; \
         private repos require the `repo` scope."
    } else {
        "your stored github token does not have the `repo` scope, \
         which is needed to list private repos."
    };
    crate::output::diag::warn_with_hint(
        prefs,
        format!(
            "no private github repos shown — {detail} (current scopes: {})",
            scopes.join(", ")
        ),
        "rupu auth logout --provider github && rupu auth login --provider github --mode sso",
    );
}
