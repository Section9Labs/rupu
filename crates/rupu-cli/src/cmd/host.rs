//! `rupu host add | list | remove` — manage named rupu-cp hosts.
//!
//! Each host is stored as a TOML file under `~/.rupu/hosts/`; tokens live in
//! the system keychain. The built-in `local` host is always shown first in
//! `list` and may not be removed.

#![deny(clippy::all)]

use anyhow::Context;
use clap::Subcommand;
use rupu_workspace::{add_bucket_host, add_ssh_host, delete_host_token, set_host_token, Host, HostStore, HostTransport};
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
    /// Mutually exclusive with `--ssh` and `--bucket`.
    #[arg(long, conflicts_with_all = ["ssh", "bucket"], required_unless_present_any = ["ssh", "bucket"])]
    pub url: Option<String>,
    /// SSH destination: `user@host` or an `~/.ssh/config` alias. Selects the
    /// Ssh transport. Mutually exclusive with `--url` and `--bucket`.
    #[arg(long, conflicts_with = "bucket")]
    pub ssh: Option<String>,
    /// SSH port override (default: 22). Only relevant with `--ssh`.
    #[arg(long)]
    pub port: Option<u16>,
    /// Path to an SSH identity file. Only relevant with `--ssh`.
    #[arg(long)]
    pub identity: Option<PathBuf>,
    /// Object-store URL for the bucket transport (`s3://…` / `gs://…` /
    /// `file://…`). Selects the Bucket transport. Mutually exclusive with
    /// `--url` and `--ssh`. Credentials come from the environment / cloud
    /// credential chain and are never stored.
    #[arg(long, conflicts_with_all = ["url", "ssh"])]
    pub bucket: Option<String>,
    /// Object-store key prefix (optional). Only relevant with `--bucket`.
    #[arg(long)]
    pub prefix: Option<String>,
    /// API token. Mutually exclusive with `--token-stdin`. Only relevant with `--url`.
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
        token_hash: None,
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

/// Add an SSH host. Returns the newly-minted host id.
///
/// No secret is stored — authentication is delegated to the system `ssh`.
pub(crate) fn add_ssh_host_cli(
    store_root: PathBuf,
    name: String,
    host: String,
    port: Option<u16>,
    identity_file: Option<PathBuf>,
) -> anyhow::Result<String> {
    let store = HostStore { root: store_root };
    let record = add_ssh_host(&store, &name, &host, port, identity_file)
        .context("save ssh host record")?;
    Ok(record.id)
}

/// Add a Bucket host. Returns the newly-minted host id.
///
/// No secret is stored — authentication is delegated to the environment /
/// cloud credential chain.
pub(crate) fn add_bucket_host_cli(
    store_root: PathBuf,
    name: String,
    url: String,
    prefix: Option<String>,
) -> anyhow::Result<String> {
    let store = HostStore { root: store_root };
    let record = add_bucket_host(&store, &name, &url, prefix)
        .context("save bucket host record")?;
    Ok(record.id)
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
            HostTransport::Tunnel { node_id } => format!("tunnel:{node_id}"),
            HostTransport::Ssh { host, port, .. } => match port {
                Some(p) => format!("ssh:{host}:{p}"),
                None => format!("ssh:{host}"),
            },
            HostTransport::Bucket { url, .. } => format!("bucket:{url}"),
        };
        rows.push((host.id, host.name, label));
    }
    Ok(rows)
}

/// Remove a host by id. Refuses the built-in `"local"` id with an error.
///
/// Keychain deletion is **best-effort**: a locked or unavailable keychain
/// produces a warning but does not fail the command after the store record
/// has already been removed — matches [`HostRegistry::remove_host`]'s
/// behaviour so the CLI and CP are consistent.
pub(crate) fn remove_host(store_root: PathBuf, id: String) -> anyhow::Result<()> {
    if id == "local" {
        anyhow::bail!("cannot remove the built-in local host");
    }
    let store = HostStore { root: store_root };
    store.delete(&id).context("delete host record")?;
    // Best-effort: a locked keychain must not fail the command post-deletion.
    if let Err(e) = delete_host_token(&id) {
        tracing::warn!(host_id = %id, error = %e, "host remove: could not delete token from keychain; continuing");
    }
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
    if let Some(ssh_dest) = args.ssh {
        // SSH transport — no token involved.
        let id = add_ssh_host_cli(hosts_dir()?, args.name, ssh_dest, args.port, args.identity)?;
        println!("{id}");
    } else if let Some(bucket_url) = args.bucket {
        // Bucket transport — no token involved; credentials from env/cloud chain.
        let id = add_bucket_host_cli(hosts_dir()?, args.name, bucket_url, args.prefix)?;
        println!("{id}");
    } else {
        // HttpCp transport.
        let url = args.url.expect("clap requires --url when --ssh and --bucket are absent");
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
        let id = add_host(hosts_dir()?, args.name, url, token)?;
        println!("{id}");
    }
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

    #[test]
    fn bucket_add_roundtrip() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("hosts");

        // -- add Bucket host (no prefix) --
        let id = add_bucket_host_cli(
            root.clone(),
            "my-bucket".into(),
            "s3://my-bucket/rupu".into(),
            None,
        )
        .expect("add_bucket_host_cli failed");
        assert!(id.starts_with("host_"), "id should start with host_: {id}");

        // -- host record must persist with correct Bucket transport --
        let store = HostStore { root: root.clone() };
        let hosts = store.list().expect("store.list failed");
        let h = hosts
            .iter()
            .find(|h| h.id == id)
            .expect("bucket host not found in store");
        assert_eq!(h.name, "my-bucket");
        assert!(h.token_hash.is_none(), "Bucket hosts must not store a token");
        match &h.transport {
            HostTransport::Bucket { url, prefix } => {
                assert_eq!(url, "s3://my-bucket/rupu");
                assert!(prefix.is_none(), "prefix should be None");
            }
            other => panic!("expected Bucket transport, got {other:?}"),
        }

        // -- add Bucket host with prefix --
        let id2 = add_bucket_host_cli(
            root.clone(),
            "my-bucket-prefixed".into(),
            "gs://my-gcs-bucket".into(),
            Some("runs/prod/".into()),
        )
        .expect("add_bucket_host_cli with prefix failed");

        let hosts2 = store.list().expect("store.list 2 failed");
        let h2 = hosts2
            .iter()
            .find(|h| h.id == id2)
            .expect("second bucket host not found");
        match &h2.transport {
            HostTransport::Bucket { url, prefix } => {
                assert_eq!(url, "gs://my-gcs-bucket");
                assert_eq!(prefix.as_deref(), Some("runs/prod/"));
            }
            other => panic!("expected Bucket transport for id2, got {other:?}"),
        }

        // -- both appear in list with correct labels --
        let rows = list_hosts(root.clone()).expect("list_hosts failed");
        let found1 = rows.iter().find(|(i, _, _)| i == &id);
        let found2 = rows.iter().find(|(i, _, _)| i == &id2);
        assert!(found1.is_some(), "host {id} missing from list");
        assert!(found2.is_some(), "host {id2} missing from list");
        assert_eq!(found1.unwrap().2, "bucket:s3://my-bucket/rupu");
        assert_eq!(found2.unwrap().2, "bucket:gs://my-gcs-bucket");
    }

    #[test]
    fn ssh_add_roundtrip() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("hosts");

        // -- add SSH host (no port, no identity) --
        let id = add_ssh_host_cli(
            root.clone(),
            "edge".into(),
            "deploy@edge.example".into(),
            None,
            None,
        )
        .expect("add_ssh_host_cli failed");
        assert!(id.starts_with("host_"), "id should start with host_: {id}");

        // -- host record must persist with correct Ssh transport --
        let store = HostStore { root: root.clone() };
        let hosts = store.list().expect("store.list failed");
        let h = hosts
            .iter()
            .find(|h| h.id == id)
            .expect("ssh host not found in store");
        assert_eq!(h.name, "edge");
        assert!(h.token_hash.is_none(), "Ssh hosts must not store a token");
        match &h.transport {
            HostTransport::Ssh { host, port, identity_file } => {
                assert_eq!(host, "deploy@edge.example");
                assert!(port.is_none(), "port should be None");
                assert!(identity_file.is_none(), "identity_file should be None");
            }
            other => panic!("expected Ssh transport, got {other:?}"),
        }

        // -- add SSH host with port + identity --
        let identity_path = PathBuf::from("/home/deploy/.ssh/id_ed25519");
        let id2 = add_ssh_host_cli(
            root.clone(),
            "edge-custom".into(),
            "deploy@edge2.example".into(),
            Some(2222),
            Some(identity_path.clone()),
        )
        .expect("add_ssh_host_cli with port+identity failed");

        let hosts2 = store.list().expect("store.list 2 failed");
        let h2 = hosts2
            .iter()
            .find(|h| h.id == id2)
            .expect("second ssh host not found");
        match &h2.transport {
            HostTransport::Ssh { host, port, identity_file } => {
                assert_eq!(host, "deploy@edge2.example");
                assert_eq!(*port, Some(2222));
                assert_eq!(identity_file.as_deref(), Some(identity_path.as_path()));
            }
            other => panic!("expected Ssh transport for id2, got {other:?}"),
        }

        // -- both appear in list with correct labels --
        let rows = list_hosts(root.clone()).expect("list_hosts failed");
        let found1 = rows.iter().find(|(i, _, _)| i == &id);
        let found2 = rows.iter().find(|(i, _, _)| i == &id2);
        assert!(found1.is_some(), "host {id} missing from list");
        assert!(found2.is_some(), "host {id2} missing from list");
        assert_eq!(found1.unwrap().2, "ssh:deploy@edge.example");
        assert_eq!(found2.unwrap().2, "ssh:deploy@edge2.example:2222");
    }
}
