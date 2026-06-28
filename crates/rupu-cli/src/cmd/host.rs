//! `rupu host add | list | remove` — manage named rupu-cp hosts.
//!
//! Each host is stored as a TOML file under `~/.rupu/hosts/`; tokens live in
//! the system keychain. The built-in `local` host is always shown first in
//! `list` and may not be removed.

#![deny(clippy::all)]

use anyhow::Context;
use clap::Subcommand;
use rupu_workspace::{delete_host_token, set_host_token, Host, HostStore, HostTransport};
use std::io::{self, Read};
use std::path::PathBuf;
use std::process::ExitCode;

// ---------------------------------------------------------------------------
// Clap types
// ---------------------------------------------------------------------------

#[derive(Subcommand, Debug)]
pub enum Action {
    /// Register a remote rupu-cp host.
    Add(AddArgs),
    /// List configured hosts (local is always shown first).
    List,
    /// Remove a host by id.
    Remove(RemoveArgs),
}

#[derive(clap::Args, Debug)]
pub struct AddArgs {
    /// Display name for the host.
    pub name: String,
    /// Base URL of the remote rupu-cp instance (e.g. https://rupu.example.com).
    #[arg(long)]
    pub url: String,
    /// API token. Mutually exclusive with `--token-stdin`.
    #[arg(long, conflicts_with = "token_stdin")]
    pub token: Option<String>,
    /// Read the API token from stdin (one line). Mutually exclusive with `--token`.
    #[arg(long)]
    pub token_stdin: bool,
}

#[derive(clap::Args, Debug)]
pub struct RemoveArgs {
    /// Id of the host to remove (e.g. `host_01J...`).
    pub id: String,
}

// ---------------------------------------------------------------------------
// Public handler (async, mirrors sibling shape)
// ---------------------------------------------------------------------------

pub async fn handle(action: Action) -> ExitCode {
    let result = match action {
        Action::Add(args) => add_inner(args),
        Action::List => list_inner(),
        Action::Remove(args) => remove_inner(args),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => crate::output::diag::fail(e),
    }
}

// ---------------------------------------------------------------------------
// Inner helpers — `pub(crate)` so tests can call them with an explicit store
// root (tempdir) and skip touching the real `~/.rupu/` or system keychain.
// ---------------------------------------------------------------------------

/// Add a host. Returns the newly-minted host id.
///
/// `token` is stored in the system keychain when `Some`; pass `None` to skip
/// (useful in test environments where keychain access is unavailable).
pub(crate) fn add_host(
    store_root: PathBuf,
    name: String,
    base_url: String,
    token: Option<String>,
) -> anyhow::Result<String> {
    let id = format!("host_{}", ulid::Ulid::new());
    let host = Host {
        id: id.clone(),
        name,
        transport: HostTransport::HttpCp { base_url },
        created_at: chrono::Utc::now().to_rfc3339(),
        last_seen_at: None,
    };
    let store = HostStore { root: store_root };
    store.save(&host).context("save host record")?;
    if let Some(tok) = token {
        set_host_token(&id, &tok).context("store token in keychain")?;
    }
    Ok(id)
}

/// List all hosts. The implicit `local` entry is always first; remote hosts
/// follow sorted by id (the order `HostStore::list` already returns).
///
/// Returns `(id, name, transport_label)` tuples.
pub(crate) fn list_hosts(
    store_root: PathBuf,
) -> anyhow::Result<Vec<(String, String, String)>> {
    let local = Host::local();
    let mut rows: Vec<(String, String, String)> =
        vec![(local.id, local.name, "local".to_string())];

    let store = HostStore { root: store_root };
    for host in store.list().context("list host records")? {
        let label = match &host.transport {
            HostTransport::Local => "local".to_string(),
            HostTransport::HttpCp { base_url } => base_url.clone(),
        };
        rows.push((host.id, host.name, label));
    }
    Ok(rows)
}

/// Remove a host by id. Refuses the built-in `"local"` id with an error.
pub(crate) fn remove_host(store_root: PathBuf, id: String) -> anyhow::Result<()> {
    if id == "local" {
        anyhow::bail!("cannot remove the built-in local host");
    }
    let store = HostStore { root: store_root };
    store.delete(&id).context("delete host record")?;
    delete_host_token(&id).context("delete host token from keychain")?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Private dispatch helpers (resolve real store root from `~/.rupu/hosts/`)
// ---------------------------------------------------------------------------

fn hosts_dir() -> anyhow::Result<PathBuf> {
    let global = crate::paths::global_dir()?;
    crate::paths::ensure_dir(&global)?;
    Ok(global.join("hosts"))
}

fn add_inner(args: AddArgs) -> anyhow::Result<()> {
    let token = if args.token_stdin {
        let mut buf = String::new();
        io::stdin().read_to_string(&mut buf)?;
        let t = buf.trim().to_string();
        if t.is_empty() {
            anyhow::bail!("--token-stdin: no token received on stdin");
        }
        Some(t)
    } else {
        args.token
    };
    let id = add_host(hosts_dir()?, args.name, args.url, token)?;
    println!("{id}");
    Ok(())
}

fn list_inner() -> anyhow::Result<()> {
    for (id, name, url) in list_hosts(hosts_dir()?)? {
        println!("{id}\t{name}\t{url}");
    }
    Ok(())
}

fn remove_inner(args: RemoveArgs) -> anyhow::Result<()> {
    remove_host(hosts_dir()?, args.id.clone())?;
    println!("removed {}", args.id);
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn add_list_remove_roundtrip() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("hosts");

        // -- add --
        let id = add_host(
            root.clone(),
            "prod".into(),
            "https://rupu.example.com".into(),
            None, // skip keychain in test environments
        )
        .expect("add_host failed");
        assert!(id.starts_with("host_"), "id should start with host_: {id}");

        // -- list --
        let rows = list_hosts(root.clone()).expect("list_hosts failed");
        // local is always first
        assert_eq!(rows[0].0, "local", "first row should be local");
        // new host must appear somewhere
        let found = rows.iter().find(|(i, _, _)| i == &id);
        assert!(found.is_some(), "host {id} not found in list");
        let (_, name, url) = found.unwrap();
        assert_eq!(name, "prod");
        assert_eq!(url, "https://rupu.example.com");

        // -- remove --
        remove_host(root.clone(), id.clone()).expect("remove_host failed");
        let rows_after = list_hosts(root.clone()).expect("list_hosts after remove failed");
        assert!(
            rows_after.iter().all(|(i, _, _)| i != &id),
            "host {id} should be gone after remove"
        );
    }

    #[test]
    fn remove_local_is_refused() {
        let dir = tempdir().unwrap();
        let err = remove_host(dir.path().join("hosts"), "local".into())
            .expect_err("remove local should fail");
        assert!(
            err.to_string().contains("local"),
            "expected refusal message, got: {err}"
        );
    }

    #[test]
    fn list_empty_store_still_has_local() {
        let dir = tempdir().unwrap();
        let rows = list_hosts(dir.path().join("hosts")).expect("list_hosts failed");
        assert_eq!(rows.len(), 1, "should have exactly one row (local)");
        assert_eq!(rows[0].0, "local");
        assert_eq!(rows[0].2, "local");
    }
}
