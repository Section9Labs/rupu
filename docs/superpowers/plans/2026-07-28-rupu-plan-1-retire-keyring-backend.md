# Plan 1: Retire the Keyring Credential Backend Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Status: IMPLEMENTED** — PR #554 (6 commits). Three corrections found during execution are marked **CORRECTION** inline below; they are kept rather than rewritten so the reasoning is auditable.

**Goal:** Make the chmod-600 file store rupu's only credential backend on every platform, and delete the keyring machinery entirely.

**Architecture:** The file backend is already the default (`resolver.rs:97-135`); this removes the alternative rather than flipping a switch. Work proceeds inward-out: collapse the resolver's two-variant `Storage` to a single file path, delete the now-unreferenced `KeyringBackend` / probe / keychain-addressing modules and fix their consumers, move host tokens to a file store, delete the `rupu-keychain-acl` crate, add a one-time notice for users whose credentials are stranded in an OS keychain, and finally drop the `keyring` workspace dependency.

**Tech Stack:** Rust 1.95 (pinned), `cargo`, `thiserror` / `anyhow`, `serde_json`, `assert_cmd`.

**Spec:** `docs/superpowers/specs/2026-07-27-rupu-cross-platform-release-design.md` §3

**Why this is its own plan:** it has standalone value — the keyring path was too complex for what it bought — and it is also a prerequisite for Plan 2, because it is what removes `libdbus-sys` from the Linux dependency graph and makes a static musl build possible.

## Global Constraints

- Rust edition 2021; toolchain pinned to **1.95** in `rust-toolchain.toml`.
- **Workspace dependencies only.** Versions pinned in the root `Cargo.toml`; never in a crate `Cargo.toml`.
- `#![deny(clippy::all)]` workspace-wide. `unsafe_code = "forbid"`. **CORRECTION:** Task 4 does *not* leave the workspace exemption-free — `rupu-app` sets `unsafe_code = "deny"` with three `#[allow]` sites for objc2/AppKit. What Task 4 achieves is narrower and still worth having: `rupu-app` is not in `rupu-cli`'s dependency graph, so every crate linked into the shipped `rupu` binary is `forbid(unsafe_code)`.
- **No mock or silent-noop code paths.** Anything that cannot work must fail loudly. This is why the keychain option is removed rather than left to degrade.
- **Do not touch credential *discovery*.** `rupu-providers` reads Claude Code's keychain entry to import credentials from it (`crates/rupu-providers/src/auth/discovery.rs:69`, `anthropic.rs::load_claude_code_keychain`). It does not use the `keyring` crate and is out of scope. Removing rupu's own keychain *storage* is not the same as removing the ability to *discover* credentials another tool left behind.
- The file backend's behavior is unchanged: `~/.rupu/auth.json`, permissions reset to 0600 on every write, `tracing::warn!` if found wider than 0600, honoring `RUPU_HOME` and `RUPU_AUTH_FILE`.
- **Never run `cargo fmt` package-wide.** `main` has 532 rustfmt diff sites. Format only files you touched: `rustfmt path/to/file.rs`.
- `rupu-app` is macOS-only; scope test commands `--workspace --exclude rupu-app` when running the whole workspace.
- Every change goes through a feature branch and a PR. Never commit directly to `main`.

---

### Task 1: Collapse the resolver to file-only storage

`KeychainResolver` currently branches on a two-variant `Storage` enum. Removing the keyring variant removes every `rupu_keychain_acl` call site in the codebase as a side effect — they all live in those match arms.

**Files:**
- Modify: `crates/rupu-auth/src/resolver.rs` (module doc at 36-51; `Storage` enum ~59; `with_service` 88-140; `entry()` ~150; keyring match arms ~187-330; `read()` ~374)
- Test: `crates/rupu-auth/tests/keychain_resolver.rs` (existing; rewrite for the file backend)

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces: `KeychainResolver { path: PathBuf }` — no `Storage` enum, no `entry()` method. Public constructors `KeychainResolver::new()` and `::with_service(&str)` keep their signatures so no caller changes; `with_service`'s argument becomes unused and is renamed `_service` with a doc note.

- [ ] **Step 1: Write the failing test**

Replace the contents of `crates/rupu-auth/tests/keychain_resolver.rs` with:

```rust
//! The resolver has exactly one storage backend: a chmod-600 JSON file.
//! These tests pin that there is no second path — asking for the OS
//! keychain by env var must not change where credentials land.
//!
//! Note these are NOT `#[ignore]`d, unlike the real-keychain test they
//! replace. Nothing touches the OS keychain any more, so there is no
//! GUI prompt to hang CI and no reason to opt out of the default run.

use rupu_auth::backend::ProviderId;
use rupu_auth::resolver::{CredentialResolver, KeychainResolver};
use rupu_auth::stored::StoredCredential;
use rupu_providers::AuthMode;
use serial_test::serial;

#[tokio::test]
#[serial]
async fn credentials_round_trip_through_the_file_store() {
    let home = assert_fs::TempDir::new().unwrap();
    std::env::set_var("RUPU_HOME", home.path());

    let r = KeychainResolver::new();
    r.store(
        ProviderId::Anthropic,
        AuthMode::ApiKey,
        &StoredCredential::api_key("sk-roundtrip"),
    )
    .await
    .expect("store");

    let (mode, creds) = r
        .get("anthropic", Some(AuthMode::ApiKey))
        .await
        .expect("get");
    assert_eq!(mode, AuthMode::ApiKey);
    match creds {
        rupu_providers::auth::AuthCredentials::ApiKey { key } => assert_eq!(key, "sk-roundtrip"),
        other => panic!("expected an ApiKey credential, got {other:?}"),
    }

    assert!(
        home.path().join("auth.json").exists(),
        "credentials must land in auth.json"
    );
}

#[tokio::test]
#[serial]
async fn requesting_the_keychain_by_env_var_still_uses_the_file_store() {
    let home = assert_fs::TempDir::new().unwrap();
    std::env::set_var("RUPU_HOME", home.path());
    std::env::set_var("RUPU_AUTH_BACKEND", "keychain");

    let r = KeychainResolver::new();
    r.store(
        ProviderId::Anthropic,
        AuthMode::ApiKey,
        &StoredCredential::api_key("sk-no-divert"),
    )
    .await
    .expect("store");

    assert!(
        home.path().join("auth.json").exists(),
        "there is no keychain backend any more; the env var must not divert storage"
    );

    std::env::remove_var("RUPU_AUTH_BACKEND");
}
```

Add `serial_test.workspace = true` to `[dev-dependencies]` in `crates/rupu-auth/Cargo.toml` — it is not currently there, and both tests mutate process-global env vars. (`rupu_providers` is a normal dependency of `rupu-auth`, and test targets see normal plus dev dependencies, so no extra manifest entry is needed for it.)

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p rupu-auth --test keychain_resolver`
Expected: FAIL — the second test writes to the OS keychain, so `auth.json` is not created.

- [ ] **Step 3: Collapse the `Storage` enum**

In `crates/rupu-auth/src/resolver.rs`, replace the `Storage` enum (~line 58-62) and the struct field with a single path:

```rust
pub struct KeychainResolver {
    /// Where credentials live: a chmod-600 JSON file. There is no
    /// second backend — see the module docs.
    path: PathBuf,
}
```

Delete the `enum Storage { .. }` declaration entirely.

- [ ] **Step 4: Simplify `with_service`**

Replace the body of `with_service` (lines 88-140, the whole backend-selection block) with:

```rust
    /// The `service` argument is retained for source compatibility with
    /// callers written against the keychain era; it no longer selects
    /// anything, because there is only one backend.
    pub fn with_service(_service: &str) -> Self {
        let path = std::env::var("RUPU_AUTH_FILE")
            .map(PathBuf::from)
            .unwrap_or_else(default_auth_json_path);
        tracing::debug!(path = %path.display(), "credential store");
        Self { path }
    }
```

- [ ] **Step 5: Delete the keyring code paths**

Delete from `crates/rupu-auth/src/resolver.rs`:
- the `entry()` method (~line 150), whose only body is the `Storage::Keyring` arm at 153
- the four remaining `Storage::Keyring { service } =>` match arms — at lines **207**, **237**, and **292**, plus the constructor arm at **143** — including all `rupu_keychain_acl::*` calls
- the `use crate::keychain_layout::...` imports at lines 31-32
- the `legacy_key_for` call in `read()` (~line 375) and the legacy-account fallback it drives

Verify you found them all: `grep -n 'Storage::Keyring' crates/rupu-auth/src/resolver.rs` must return nothing.

Each surviving `Storage::JsonFile { path } =>` arm (lines 141, 226, 259, 357) becomes straight-line code against `self.path`, with the `match` removed entirely.

- [ ] **Step 6: Rewrite the module doc**

Replace the stale module doc at `crates/rupu-auth/src/resolver.rs:36-51` with:

```rust
/// Production resolver: reads/writes [`StoredCredential`] JSON to a
/// chmod-600 file at `~/.rupu/auth.json` (overridable via `RUPU_HOME`
/// or `RUPU_AUTH_FILE`).
///
/// This is the only credential backend. The OS keychain was retired
/// because a bare CLI binary's keychain requirement is cdhash-bound:
/// every rebuild invalidates it and the next read silently fails, which
/// is how "my credentials vanished after an update" kept happening.
/// `gh`, `aws`, `gcloud`, `kubectl`, and `terraform` all store
/// credentials in files for the same reason.
///
/// On SSO entries whose access token is within [`EXPIRY_REFRESH_BUFFER_SECS`]
/// of expiry, [`KeychainResolver::get`] performs a silent token refresh via
/// the standard OAuth refresh-token grant before returning credentials.
```

- [ ] **Step 7: Run the tests to verify they pass**

Run: `cargo test -p rupu-auth`
Expected: PASS. `crates/rupu-auth/src/keyring.rs` still compiles at this point — it is deleted in Task 2.

- [ ] **Step 8: Format and commit**

```bash
rustfmt crates/rupu-auth/src/resolver.rs crates/rupu-auth/tests/keychain_resolver.rs
git add crates/rupu-auth/src/resolver.rs crates/rupu-auth/tests/keychain_resolver.rs
git commit -m "refactor(auth): resolver stores credentials in one place

Collapses the two-variant Storage enum to a single chmod-600 file path.
Removes every rupu_keychain_acl call site as a side effect — they all
lived in the keyring match arms."
```

---

### Task 2: Delete `KeyringBackend`, the probe, and the keychain addressing layer

With the resolver collapsed, these modules have no remaining callers except each other and `rupu auth backend`.

**Files:**
- Delete: `crates/rupu-auth/src/keyring.rs`, `crates/rupu-auth/src/probe.rs`, `crates/rupu-auth/tests/keyring_ignored.rs`, `crates/rupu-auth/tests/probe_cache.rs`
- **CORRECTION — do NOT delete `crates/rupu-auth/src/keychain_layout.rs`.** `key_for(..).account` produces `"anthropic/api-key"`, which is the **on-disk key format of `auth.json`**, not merely a keychain address. Deleting it silently changes the key format and orphans every existing user's credentials. Rename it to `account_key`, have it return plain `String`s, and add a test pinning the exact key strings as a compatibility contract.
- Modify: `crates/rupu-auth/src/lib.rs` (whole file — module list, exports, and stale module doc)
- Modify: `crates/rupu-auth/src/backend.rs:60` (`AuthError::Keyring`)
- Modify: `crates/rupu-cli/src/cmd/auth.rs:420-510` (the `backend` function)

**Interfaces:**
- Consumes: Task 1's collapsed resolver.
- Produces: `rupu_auth` exports `AuthBackend`, `AuthError`, `ProviderId`, `JsonFileBackend`, `StoredCredential`, `CredentialResolver`, `KeychainResolver`. `BackendChoice`, `ProbeCache`, `select_backend`, `ENV_BACKEND_OVERRIDE`, `KeyringBackend`, `key_for`, `legacy_key_for`, `KeychainKey` no longer exist. `rupu auth backend --use keychain` exits non-zero with a message containing "no longer supported".

- [ ] **Step 1: Write the failing test**

Create `crates/rupu-cli/tests/cli_auth_backend_retired.rs`:

```rust
//! `rupu auth backend` survives as a reporting command, but the keychain
//! is gone. Asking for it must fail loudly: silently writing to a
//! plaintext file while the user believes they selected an OS keystore
//! would be a security-relevant lie.

use assert_cmd::Command;

#[test]
fn requesting_the_keychain_is_an_explicit_error() {
    Command::cargo_bin("rupu")
        .expect("rupu binary builds")
        .args(["auth", "backend", "--use", "keychain"])
        .assert()
        .failure()
        .stderr(predicates::str::contains("no longer supported"));
}

#[test]
fn requesting_the_file_backend_succeeds() {
    Command::cargo_bin("rupu")
        .expect("rupu binary builds")
        .args(["auth", "backend", "--use", "file"])
        .assert()
        .success();
}

#[test]
fn reporting_the_backend_still_works() {
    Command::cargo_bin("rupu")
        .expect("rupu binary builds")
        .args(["auth", "backend"])
        .assert()
        .success()
        .stdout(predicates::str::contains("file"));
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p rupu-cli --test cli_auth_backend_retired`
Expected: FAIL — `--use keychain` currently succeeds and writes `BackendChoice::Keyring` to the probe cache.

- [ ] **Step 3: Delete the modules**

```bash
git rm crates/rupu-auth/src/keyring.rs \
       crates/rupu-auth/src/probe.rs \
       crates/rupu-auth/src/keychain_layout.rs \
       crates/rupu-auth/tests/keyring_ignored.rs
```

- [ ] **Step 4: Rewrite `crates/rupu-auth/src/lib.rs`**

Replace the whole file with:

```rust
//! rupu-auth — credential storage.
//!
//! One backend: [`JsonFileBackend`] stores secrets in `~/.rupu/auth.json`
//! with permissions enforced to mode 0600, honoring `RUPU_HOME` and
//! `RUPU_AUTH_FILE`.
//!
//! The OS keychain backend was retired. A bare CLI binary's keychain
//! requirement is cdhash-bound, so every rebuild invalidated it and the
//! next read failed silently — the "credentials vanished after an
//! update" failure. Peer CLIs (`gh`, `aws`, `gcloud`, `kubectl`,
//! `terraform`) all use files for the same reason.
//!
//! Note this is about where rupu stores *its own* credentials. Importing
//! credentials another tool left in a keychain is a separate concern and
//! lives in `rupu-providers` (`auth::discovery`).

pub mod backend;
pub mod json_file;

pub mod oauth;

pub mod stored;
pub use stored::StoredCredential;

pub mod in_memory;
pub mod resolver;
pub use resolver::{CredentialResolver, KeychainResolver};

pub use backend::{AuthBackend, AuthError, ProviderId};
pub use json_file::JsonFileBackend;
```

Note this also drops the stale scaffolding comment ("Real implementations land in Tasks 13-15") left over from the crate's original build-out.

- [ ] **Step 5: Drop the error variant**

In `crates/rupu-auth/src/backend.rs`, delete the `Keyring` variant at line 60 and its `#[error("keyring: {0}")]` attribute.

- [ ] **Step 6: Rewrite `rupu auth backend`**

In `crates/rupu-cli/src/cmd/auth.rs`, in `async fn backend` (line 420):

- Delete the `ProbeCache` construction (line 434) and every `cache.write(...)` call.
- Replace the `--use` match (lines 440-445) with:

```rust
        let target_norm = target.trim().to_ascii_lowercase();
        match target_norm.as_str() {
            "file" | "json" | "json-file" | "json_file" => {}
            "keyring" | "keychain" | "os" | "os-keychain" => anyhow::bail!(
                "the OS keychain backend is no longer supported — rupu stores \
                 credentials in a chmod-600 file at `{}`. There is nothing to \
                 select; `--use file` is the only valid value and is already \
                 in effect.",
                auth_path.display()
            ),
            other => anyhow::bail!("unknown backend `{other}` — the only supported value is: file"),
        }
```

- Replace every remaining `rupu_auth::BackendChoice::*` reference (lines 451-452, 489-490, 502-503) so the reported `active_backend` is always `"file"` and `cache_choice` / `env_override` are `None`. Keep the `AuthBackendReport` struct shape unchanged so the JSON output contract does not break.

- [ ] **Step 7: Delete the stale probe cache on sight**

Still in `async fn backend`, after the `--use` handling, add:

```rust
    // The probe cache selected between backends that no longer both
    // exist. Remove it rather than leaving a file that implies a choice
    // is still being made.
    let stale_cache = global.join("cache/auth-backend.json");
    if stale_cache.exists() {
        if let Err(e) = std::fs::remove_file(&stale_cache) {
            tracing::warn!(error = %e, path = %stale_cache.display(), "could not remove stale probe cache");
        }
    }
```

- [ ] **Step 8: Run the tests to verify they pass**

```bash
cargo test -p rupu-auth
cargo test -p rupu-cli --test cli_auth_backend_retired
cargo build --workspace --exclude rupu-app
```
Expected: all PASS. Fix any other consumer the compiler surfaces — `grep -rn 'BackendChoice\|ProbeCache\|select_backend\|KeyringBackend\|key_for' crates/` should return nothing outside test fixtures once done.

- [ ] **Step 9: Format and commit**

```bash
rustfmt crates/rupu-auth/src/lib.rs crates/rupu-auth/src/backend.rs \
        crates/rupu-cli/src/cmd/auth.rs crates/rupu-cli/tests/cli_auth_backend_retired.rs
git add -A
git commit -m "feat(auth)!: retire the OS keychain backend

Deletes KeyringBackend, the backend probe, and the keychain addressing
layer. \`rupu auth backend --use keychain\` now fails with an explicit
message rather than silently writing to a file the user did not choose."
```

---

### Task 3: Move host tokens to a file store

`rupu-workspace` keeps CP host tokens in the keychain via three small helpers. They move to a chmod-600 file alongside the host registry.

**Files:**
- Modify: `crates/rupu-workspace/src/host_store.rs` (module doc 1-6; `HostStoreError::Keyring` 39-40; the three helpers 331-360)
- Modify: `crates/rupu-workspace/Cargo.toml:19` (drop `keyring`)

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces: `set_host_token(host_id: &str, token: &str) -> Result<(), HostStoreError>`, `get_host_token(host_id: &str) -> Result<Option<String>, HostStoreError>`, `delete_host_token(host_id: &str) -> Result<(), HostStoreError>` — signatures unchanged, so no caller changes. Tokens live in `<RUPU_HOME>/hosts/tokens.json`, mode 0600.

- [ ] **Step 1: Write the failing test**

Add to the existing `mod tests` at the bottom of `crates/rupu-workspace/src/host_store.rs`:

```rust
    #[test]
    #[serial_test::serial]
    fn host_tokens_round_trip_through_a_chmod_600_file() {
        let home = assert_fs::TempDir::new().unwrap();
        std::env::set_var("RUPU_HOME", home.path());

        set_host_token("mini", "tok-abc").unwrap();
        assert_eq!(get_host_token("mini").unwrap().as_deref(), Some("tok-abc"));

        let path = home.path().join("hosts/tokens.json");
        assert!(path.exists(), "tokens must land in hosts/tokens.json");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "token file must be chmod 600");
        }

        delete_host_token("mini").unwrap();
        assert_eq!(get_host_token("mini").unwrap(), None);
    }

    #[test]
    #[serial_test::serial]
    fn missing_token_is_none_not_an_error() {
        let home = assert_fs::TempDir::new().unwrap();
        std::env::set_var("RUPU_HOME", home.path());
        assert_eq!(get_host_token("never-stored").unwrap(), None);
    }
```

Add `serial_test.workspace = true` to `[dev-dependencies]` in `crates/rupu-workspace/Cargo.toml` if not already present.

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p rupu-workspace host_token`
Expected: FAIL — tokens go to the OS keychain, so `hosts/tokens.json` is never created.

- [ ] **Step 3: Replace the three helpers**

In `crates/rupu-workspace/src/host_store.rs`, replace the whole "Keyring helpers" section (lines 331-360, including the `KEYRING_SERVICE` const) with:

```rust
// ---------------------------------------------------------------------------
// Token storage
// ---------------------------------------------------------------------------
//
// A chmod-600 JSON map at `<RUPU_HOME>/hosts/tokens.json`. Deliberately
// separate from rupu-auth's `JsonFileBackend`, which is keyed by the
// fixed `ProviderId` enum — host ids are arbitrary strings, a different
// key space. The permission discipline is the same.

use std::collections::BTreeMap;

fn tokens_path() -> PathBuf {
    rupu_home().join("hosts").join("tokens.json")
}

fn read_tokens() -> Result<BTreeMap<String, String>, HostStoreError> {
    let path = tokens_path();
    match std::fs::read_to_string(&path) {
        Ok(text) => Ok(serde_json::from_str(&text).unwrap_or_default()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
        Err(source) => Err(HostStoreError::Io {
            action: format!("read {}", path.display()),
            source,
        }),
    }
}

fn write_tokens(map: &BTreeMap<String, String>) -> Result<(), HostStoreError> {
    let path = tokens_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| HostStoreError::Io {
            action: format!("create {}", parent.display()),
            source,
        })?;
    }
    let text = serde_json::to_string_pretty(map).expect("BTreeMap<String, String> serializes");
    std::fs::write(&path, text).map_err(|source| HostStoreError::Io {
        action: format!("write {}", path.display()),
        source,
    })?;
    // Reset permissions on every write, exactly as rupu-auth's json_file
    // backend does — a token file that is group- or world-readable is a
    // credential leak.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).map_err(
            |source| HostStoreError::Io {
                action: format!("chmod 600 {}", path.display()),
                source,
            },
        )?;
    }
    Ok(())
}

/// Store a token for `host_id`.
pub fn set_host_token(host_id: &str, token: &str) -> Result<(), HostStoreError> {
    let mut map = read_tokens()?;
    map.insert(host_id.to_string(), token.to_string());
    write_tokens(&map)
}

/// Retrieve the token for `host_id`. `Ok(None)` when none is stored.
pub fn get_host_token(host_id: &str) -> Result<Option<String>, HostStoreError> {
    Ok(read_tokens()?.get(host_id).cloned())
}

/// Delete the token for `host_id`. Succeeds when none is stored.
pub fn delete_host_token(host_id: &str) -> Result<(), HostStoreError> {
    let mut map = read_tokens()?;
    if map.remove(host_id).is_some() {
        write_tokens(&map)?;
    }
    Ok(())
}
```

If `host_store.rs` has no existing `rupu_home()` helper, add one mirroring `crates/rupu-auth/src/resolver.rs:69-76` — honor `RUPU_HOME` first, then `dirs::home_dir().join(".rupu")`. Reuse whatever the file already has rather than adding a second path resolver.

- [ ] **Step 4: Drop the error variant and the dependency**

Delete `HostStoreError::Keyring` (lines 39-40). Delete `keyring = { workspace = true }` from `crates/rupu-workspace/Cargo.toml:19`.

- [ ] **Step 5: Correct the module doc**

`crates/rupu-workspace/src/host_store.rs:4` says "Tokens are kept in the system keychain via `keyring`". Replace with a line stating tokens are kept in a chmod-600 JSON file at `<RUPU_HOME>/hosts/tokens.json`.

- [ ] **Step 6: Run the tests to verify they pass**

```bash
cargo test -p rupu-workspace
cargo build --workspace --exclude rupu-app
```
Expected: PASS.

- [ ] **Step 7: Format and commit**

```bash
rustfmt crates/rupu-workspace/src/host_store.rs
git add crates/rupu-workspace/
git commit -m "refactor(workspace): host tokens move to a chmod-600 file

Same permission discipline as rupu-auth's json_file backend; separate
store because host ids are arbitrary strings, not the ProviderId enum."
```

---

### Task 4: Delete the `rupu-keychain-acl` crate

Its only consumer was the resolver's keyring arms, removed in Task 1.

**CORRECTION:** this does not leave the workspace exemption-free — `rupu-app` keeps one for objc2/AppKit FFI. It does make every crate in the *shipped binary* `forbid(unsafe_code)`, which is the claim to make.

**Files:**
- Delete: `crates/rupu-keychain-acl/` (entire directory, 585 lines)
- Modify: root `Cargo.toml:20` (workspace member list)
- Modify: `crates/rupu-auth/Cargo.toml:31` (path dependency)
- Modify: `CLAUDE.md` (the crate's entry in the Crates list)

**Interfaces:**
- Consumes: Task 1 (which removed the last call sites).
- Produces: no `rupu-keychain-acl` crate. Every crate in `rupu-cli`'s dependency graph is `forbid(unsafe_code)`; `rupu-app` retains its own exemption and is out of that graph.

- [ ] **Step 1: Verify there are no remaining call sites**

Run: `grep -rn 'rupu_keychain_acl\|rupu-keychain-acl' crates/ Cargo.toml`
Expected: only the two manifest lines (root member list, `rupu-auth`'s path dep) and the `CLAUDE.md` entry. Any `.rs` hit means Task 1 is incomplete — go finish it rather than deleting a crate that is still used.

- [ ] **Step 2: Delete the crate and its references**

```bash
git rm -r crates/rupu-keychain-acl
```

Remove `"crates/rupu-keychain-acl",` from the `members` list in the root `Cargo.toml` (line 20), and remove `rupu-keychain-acl = { path = "../rupu-keychain-acl" }` from `crates/rupu-auth/Cargo.toml` (line 31).

- [ ] **Step 3: Verify the shipped binary's graph is exemption-free**

Run: `grep -rn 'allow(unsafe_code)\|unsafe_code' crates/ Cargo.toml`
Expected: the root `unsafe_code = "forbid"`, plus `rupu-app`'s own `deny` + three `#[allow]` sites — and nothing else. Then confirm `rupu-app` is outside the shipped graph:
`cargo tree -p rupu-cli -e normal --prefix none | grep 'rupu-app'`
Expected: only `rupu-app-canvas` (pure Rust, workspace-`forbid`), never `rupu-app`.

- [ ] **Step 4: Update `CLAUDE.md`**

Delete the `**rupu-keychain-acl**` bullet from the Crates list. In its place, nothing — but check whether any other CLAUDE.md text describes the keychain credential flow and correct it if so.

- [ ] **Step 5: Verify the workspace builds and tests**

```bash
cargo build --workspace --exclude rupu-app
cargo test --workspace --exclude rupu-app
```
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "chore: delete rupu-keychain-acl

Its only consumer was the resolver's keyring arms. The workspace now has
every crate in the shipped binary is now forbid(unsafe_code). rupu-app
keeps its own exemption for objc2 FFI and is not in that graph."
```

---

### Task 5: One-time notice for credentials stranded in an OS keychain

There is no keychain→file migration path, so a user whose credentials are still in the keychain would simply find themselves logged out. Detection is cheap and turns a confusing failure into an instruction.

**Files:**
- Create: `crates/rupu-auth/src/stranded.rs`
- Modify: `crates/rupu-auth/src/lib.rs` (register the module and export the check)
- Modify: `crates/rupu-cli/src/cmd/auth.rs` (call it from the login path)

**Interfaces:**
- Consumes: Task 2's slimmed `lib.rs`.
- Produces: `rupu_auth::stranded::detect_stranded_keychain_credentials() -> Vec<String>` returning provider names found in the legacy keychain. Empty on every non-macOS platform and whenever nothing is found.

- [ ] **Step 1: Write the failing test**

Create `crates/rupu-auth/src/stranded.rs` with the test module only, so the test names the function before it exists:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detection_is_empty_off_macos() {
        // The probe shells out to `security`, which exists only on macOS.
        // Everywhere else this must be a no-op returning no findings —
        // never an error, never a spurious warning.
        if std::env::consts::OS != "macos" {
            assert!(detect_stranded_keychain_credentials().is_empty());
        }
    }

    #[test]
    fn detection_never_panics_on_the_host_it_runs_on() {
        // Whatever the host, the probe must be total: a missing binary,
        // a locked keychain, and a denied prompt are all "no findings",
        // not a crash in the middle of `rupu auth login`.
        let _ = detect_stranded_keychain_credentials();
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p rupu-auth stranded`
Expected: FAIL — `crates/rupu-auth/src/stranded.rs` is not a registered module and the function does not exist.

- [ ] **Step 3: Write the implementation**

Prepend to `crates/rupu-auth/src/stranded.rs`:

```rust
//! Detects credentials left behind in the macOS keychain by rupu
//! versions that stored them there.
//!
//! Deliberately a `security` shellout rather than a `keyring`
//! dependency: keeping the keyring crate alive purely for migration
//! would preserve exactly the complexity its removal exists to shed.
//! Read-only — nothing is imported or deleted. The user re-runs
//! `rupu auth login`, which writes to the file store.

/// Providers whose credentials rupu used to store under the `rupu`
/// keychain service. Matches the historical account naming.
const LEGACY_PROVIDERS: &[&str] = &[
    "anthropic",
    "openai",
    "gemini",
    "copilot",
    "github",
    "gitlab",
    "linear",
];

/// Provider names with credentials still sitting in the macOS keychain.
///
/// Empty on every other OS, and empty on macOS when nothing is found or
/// the keychain cannot be read. Total by construction: this runs inside
/// `rupu auth login`, where a panic or an error would be far worse than
/// a missed notice.
pub fn detect_stranded_keychain_credentials() -> Vec<String> {
    if std::env::consts::OS != "macos" {
        return Vec::new();
    }

    LEGACY_PROVIDERS
        .iter()
        .filter(|provider| {
            std::process::Command::new("security")
                .args(["find-generic-password", "-s", "rupu", "-a"])
                .arg(provider)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
        })
        .map(|p| (*p).to_string())
        .collect()
}
```

Before finalizing `LEGACY_PROVIDERS`, confirm the historical account strings against `ProviderId::as_str()` in `crates/rupu-auth/src/backend.rs` and against the `parse_provider` match in the pre-Task-1 `resolver.rs` (recoverable via `git show HEAD~N:crates/rupu-auth/src/resolver.rs`). A wrong account name means the probe silently finds nothing, which is the one failure mode that would go unnoticed.

- [ ] **Step 4: Register the module**

Add to `crates/rupu-auth/src/lib.rs`:

```rust
pub mod stranded;
pub use stranded::detect_stranded_keychain_credentials;
```

- [ ] **Step 5: Run the test to verify it passes**

Run: `cargo test -p rupu-auth stranded`
Expected: PASS.

- [ ] **Step 6: Wire the notice into the login path**

In `crates/rupu-cli/src/cmd/auth.rs`, at the start of the login command handler, add:

```rust
    let stranded = rupu_auth::detect_stranded_keychain_credentials();
    if !stranded.is_empty() {
        eprintln!(
            "note: credentials for {} are still in the macOS keychain from an \
             older rupu. That store was retired — rupu now keeps credentials in \
             a chmod-600 file. Re-authenticating here replaces them; the old \
             keychain entries are left untouched and can be removed with:\n  \
             security delete-generic-password -s rupu -a <provider>",
            stranded.join(", ")
        );
    }
```

Place it before any provider selection so it is seen regardless of which provider the user is logging into.

- [ ] **Step 7: Verify it does not fire spuriously**

Run: `cargo run -p rupu-cli -- auth backend`
Expected: no stranded-credential notice (that command is not the login path), and no `security` prompt.

Then run the login help path and confirm the notice appears only when a legacy entry genuinely exists:
```bash
security add-generic-password -s rupu -a anthropic -w test-stranded-value
cargo run -p rupu-cli -- auth login --help >/dev/null
security delete-generic-password -s rupu -a anthropic
```

- [ ] **Step 8: Format and commit**

```bash
rustfmt crates/rupu-auth/src/stranded.rs crates/rupu-auth/src/lib.rs crates/rupu-cli/src/cmd/auth.rs
git add crates/rupu-auth/src/stranded.rs crates/rupu-auth/src/lib.rs crates/rupu-cli/src/cmd/auth.rs
git commit -m "feat(auth): notice for credentials stranded in the old keychain

Read-only \`security\` probe, no keyring dependency. Turns a silent
logged-out surprise into an instruction."
```

---

### Task 6: Remove the `keyring` workspace dependency

The last step: with no consumers left, the dependency itself goes, and the graph is verified clean on all three platforms.

**Files:**
- Modify: root `Cargo.toml:103-104`
- Modify: `Cargo.lock` (regenerated)

**Interfaces:**
- Consumes: Tasks 1-5.
- Produces: no `keyring`, `dbus-secret-service`, or `libdbus-sys` anywhere in the workspace dependency graph, on any target.

- [ ] **Step 1: Write the failing check**

```bash
for t in aarch64-apple-darwin x86_64-unknown-linux-musl x86_64-pc-windows-msvc; do
  echo "=== $t ==="
  cargo tree --workspace -e normal --prefix none --target "$t" \
    | grep -E '^(keyring|libdbus-sys|dbus-secret-service) ' | sort -u
done
```
Expected now: `keyring` on all three targets, plus `dbus-secret-service` and `libdbus-sys` on the Linux target — i.e. the check fails.

- [ ] **Step 2: Remove the dependency**

Delete these two lines from the root `Cargo.toml` (the `# Auth` comment and the entry at line 104):

```toml
# Auth
keyring = { version = "3", features = ["apple-native", "windows-native", "sync-secret-service", "vendored"] }
```

- [ ] **Step 3: Regenerate the lockfile**

Run: `cargo update --workspace --offline 2>/dev/null || cargo check --workspace --exclude rupu-app`
Expected: `Cargo.lock` updates, dropping `keyring` and its transitive `dbus-*` crates.

- [ ] **Step 4: Run the check to verify it passes**

Re-run the Step 1 loop.
Expected: no output under any of the three targets.

- [ ] **Step 5: Verify the whole workspace still builds and tests**

```bash
cargo build --workspace --exclude rupu-app
cargo test --workspace --exclude rupu-app
cargo clippy --workspace --exclude rupu-app --all-targets -- -D warnings
```
Expected: all PASS.

- [ ] **Step 6: Confirm credential discovery still works**

```bash
grep -rn 'load_claude_code_keychain' crates/rupu-providers/src/ | head -3
cargo test -p rupu-providers discovery
```
Expected: the function is still present and its tests PASS. Importing credentials another tool left in a keychain is explicitly out of scope for this plan; if this broke, something went too far.

- [ ] **Step 7: Commit**

```bash
git add Cargo.toml Cargo.lock
git commit -m "build: drop the keyring dependency

No consumers remain. Removes dbus-secret-service and libdbus-sys from
the Linux graph, which is what makes a static musl build possible
(see Plan 2)."
```

---

## Self-Review

**Spec coverage.** Spec §3 lists nine removal targets; each maps to a task:

| Spec §3 target | Task |
|---|---|
| `crates/rupu-keychain-acl/` (whole crate) | 4 |
| `crates/rupu-auth/src/keyring.rs` | 2 |
| `crates/rupu-auth/src/keychain_layout.rs` | 2 |
| `crates/rupu-auth/src/probe.rs` | 2 |
| `crates/rupu-auth/src/resolver.rs` — `Storage` collapse | 1 |
| `crates/rupu-auth/src/backend.rs:60` — `AuthError::Keyring` | 2 |
| `crates/rupu-workspace/src/host_store.rs:331-360` | 3 |
| `crates/rupu-auth/tests/keyring_ignored.rs` | 2 |
| root `Cargo.toml:104` — the dependency | 6 |

Other §3 requirements: the stale `resolver.rs` doc comment → Task 1 Step 6; the unsafe-exemption side effect → Task 4 Step 3; "what explicitly stays" (`rupu-providers` discovery) → Global Constraints and Task 6 Step 6; clean-break migration with a detection notice → Task 5.

**Placeholder scan.** No "TBD"/"TODO"/"implement later". Two steps require reading existing code before writing rather than transcribing a literal — Task 1 Step 1 (match the real `CredentialResolver` trait signatures) and Task 5 Step 3 (confirm historical provider account strings) — both name exactly which file and symbol to read and why guessing is unsafe.

**Type consistency.** `KeychainResolver::new()` / `::with_service(&str)` keep their signatures throughout (Task 1). The three `*_host_token` function signatures are unchanged across Task 3, so no caller in `rupu-cp` or `rupu-cli` needs editing. `detect_stranded_keychain_credentials() -> Vec<String>` is defined in Task 5 Step 3 and consumed by name in Steps 4 and 6. `HostStoreError::Io { action, source }` is used in Task 3's new helpers and matches the existing variant at `host_store.rs:25-30`.

**Naming note.** `KeychainResolver` becomes a misnomer once there is no keychain, but it is public API used across several crates. Renaming it is a mechanical follow-up worth doing separately, not mid-removal — it would bury the substantive diff under a rename. Flagged here rather than silently left.
