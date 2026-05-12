# Slice D — Plan 1: Workspace Shell Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship the first runnable `rupu.app` — a macOS desktop binary that opens, persists, and re-opens workspaces; renders the 180px minimal accordion sidebar populated from project + global asset discovery; and surfaces a menubar status item. No tabs, no canvas, no orchestrator work yet — that's D-2 and D-3.

**Architecture:** New crate `rupu-app` (a GPUI binary). Pure-data layers (manifest schema, storage paths, asset discovery, recents list) are unit-tested in isolation. The GPUI window / sidebar / menubar layers are thin wrappers that consume the data layers. Workspace metadata persists under `~/Library/Application Support/rupu.app/`; project trees stay clean. Single tokio runtime owned by the app for future orchestrator work; for D-1 it just exists.

**Tech Stack:** Rust 2021, GPUI (git-pinned from zed-industries/zed), `toml` 0.8, `ulid` 1.x, `chrono` 0.4, `directories` 5.x, `serde` 1.x. `objc2` for the macOS menubar shim. `notify` deferred to a later slice.

---

## Spec reference

`docs/superpowers/specs/2026-05-11-rupu-slice-d-app-design.md`

D-1 scope (from spec §10): "One window per workspace, sidebar accordion, menubar badge stub, workspace manifest + persistence. No tab content."

Out of scope for this plan (covered by later sub-slices):
- Tab content of any kind (D-2 Graph view, D-3 run viewer, D-5 YAML view, D-6 Canvas view, D-7 agent editor)
- Pane splits (introduced in D-2 alongside the first real tab content)
- `WorkflowExecutor` / `EventSink` traits (D-3)
- File > Open Recent menu (the data layer exists in D-1; the actual menu wiring lands in D-2 once we have something useful to open into)
- Workspace creation modal (D-2 — D-1 ships `File > Open Workspace…` which is enough to validate the shell)
- Repo / Issue connector wiring (D-9)
- New Workspace template picker (D-2)

---

## File structure

New crate at `crates/rupu-app/`:

```
crates/rupu-app/
  Cargo.toml                              # binary + dependencies
  src/
    main.rs                               # entry point, tokio runtime + GPUI app boot
    palette.rs                            # color tokens lifted from rupu-cli/output/palette.rs
    workspace/
      mod.rs                              # re-exports + Workspace handle struct
      manifest.rs                         # WorkspaceManifest + WorkspaceColor + RepoBinding types
      storage.rs                          # path resolution + manifest load/save
      discovery.rs                        # asset discovery walks .rupu/ dirs
      recents.rs                          # recent-workspaces list (newest-first)
    window/
      mod.rs                              # WorkspaceWindow GPUI view
      titlebar.rs                         # color chip + name + in-flight count (always 0 for D-1)
      sidebar.rs                          # minimal accordion, 5 sections, click-to-select
    menu/
      app_menu.rs                         # File menu wiring (New / Open)
      menubar.rs                          # NSStatusItem stub via objc2 (always shows 0)
  tests/
    workspace_manifest_roundtrip.rs       # integration test for manifest load/save
    discovery_walks_dirs.rs               # integration test for asset discovery
```

Existing files modified:
- `Cargo.toml` (workspace root) — add `directories`, `gpui`, `objc2*` to `[workspace.dependencies]`; add `rupu-app` to `members`.

Total new files: 14. Total tasks below: 16 (one per file approximately, plus deps, smoke test, docs, gates).

---

## Task 1: Workspace dependencies

**Files:**
- Modify: `Cargo.toml` (workspace root, `[workspace.dependencies]` table and `[workspace.members]` list)

- [ ] **Step 1: Pick a GPUI git pin**

Pick the most recent commit on `main` from `zed-industries/zed` that builds cleanly with our MSRV (`1.77`).

```bash
git ls-remote https://github.com/zed-industries/zed.git HEAD
# Note the SHA; use it in the next step
```

Expected: a 40-char SHA printed.

- [ ] **Step 2: Add workspace deps**

Open `Cargo.toml` (workspace root). In `[workspace.dependencies]`, add:

```toml
# Native macOS app (Slice D)
gpui = { git = "https://github.com/zed-industries/zed.git", rev = "PICK_SHA_FROM_STEP_1" }
directories = "5"
objc2 = "0.5"
objc2-app-kit = "0.2"
objc2-foundation = "0.2"
```

In `[workspace]`, add `"crates/rupu-app"` to the `members` list (alphabetical position is fine).

- [ ] **Step 3: Verify workspace still parses**

Run: `cargo metadata --no-deps --format-version 1 > /dev/null`
Expected: exits 0 (the new crate doesn't exist yet, so cargo will warn but still parse).

- [ ] **Step 4: Commit**

```bash
git add Cargo.toml
git commit -m "deps: add gpui + objc2 + directories for rupu-app (Slice D Plan 1)"
```

---

## Task 2: Scaffold `rupu-app` crate

**Files:**
- Create: `crates/rupu-app/Cargo.toml`
- Create: `crates/rupu-app/src/main.rs`

- [ ] **Step 1: Create the crate Cargo.toml**

```toml
# crates/rupu-app/Cargo.toml
[package]
name = "rupu-app"
version.workspace = true
edition.workspace = true
license.workspace = true
authors.workspace = true
publish = false

[[bin]]
name = "rupu-app"
path = "src/main.rs"

[dependencies]
chrono.workspace = true
directories.workspace = true
gpui.workspace = true
serde = { workspace = true, features = ["derive"] }
toml.workspace = true
ulid.workspace = true
anyhow.workspace = true
tracing.workspace = true
tracing-subscriber.workspace = true

[target.'cfg(target_os = "macos")'.dependencies]
objc2.workspace = true
objc2-app-kit.workspace = true
objc2-foundation.workspace = true

[lints]
workspace = true
```

- [ ] **Step 2: Create a minimal `main.rs`**

```rust
// crates/rupu-app/src/main.rs
//! rupu.app — native macOS desktop app.
//!
//! See `docs/superpowers/specs/2026-05-11-rupu-slice-d-app-design.md`.

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter("rupu_app=debug,gpui=info")
        .init();
    tracing::info!("rupu.app starting");
    // GPUI boot lands in Task 10.
}
```

- [ ] **Step 3: Verify it builds**

Run: `cargo build -p rupu-app`
Expected: builds clean (gpui will pull in a lot of deps; first build may take several minutes).

- [ ] **Step 4: Commit**

```bash
git add crates/rupu-app/Cargo.toml crates/rupu-app/src/main.rs
git commit -m "feat(rupu-app): scaffold crate + minimal main.rs"
```

---

## Task 3: Palette module

**Files:**
- Create: `crates/rupu-app/src/palette.rs`
- Modify: `crates/rupu-app/src/main.rs` (add `mod palette;`)

- [ ] **Step 1: Write the palette tests first (TDD)**

```rust
// crates/rupu-app/src/palette.rs
//! Color tokens for rupu.app. Mirrors the Okesu palette already in
//! rupu-cli/src/output/palette.rs so terminal output and the app
//! render the same colors. GPUI uses `Rgba` / `Hsla` types from its
//! color module; we expose `gpui::Rgba` constants here.

use gpui::Rgba;

/// Construct an opaque RGB color from 8-bit components. GPUI's `Rgba`
/// uses normalized floats internally, so we divide by 255.0.
const fn rgb(r: u8, g: u8, b: u8) -> Rgba {
    Rgba {
        r: r as f32 / 255.0,
        g: g as f32 / 255.0,
        b: b as f32 / 255.0,
        a: 1.0,
    }
}

// ── Status colors ─────────────────────────────────────────────────────────
pub const RUNNING:   Rgba = rgb(59, 130, 246);  // blue-500
pub const COMPLETE:  Rgba = rgb(34, 197, 94);   // green-500
pub const FAILED:    Rgba = rgb(239, 68, 68);   // red-500
pub const AWAITING:  Rgba = rgb(251, 191, 36);  // amber-400
pub const SKIPPED:   Rgba = rgb(203, 213, 225); // slate-300

// ── Chrome ────────────────────────────────────────────────────────────────
pub const DIM:       Rgba = rgb(100, 116, 139); // slate-500
pub const BRAND:     Rgba = rgb(124, 58, 237);  // brand-500 (purple)
pub const BRAND_300: Rgba = rgb(167, 139, 250); // brand-300 (lighter purple)

// ── Window chrome ─────────────────────────────────────────────────────────
pub const BG_PRIMARY:   Rgba = rgb(15, 15, 18);   // window background (#0f0f12)
pub const BG_SIDEBAR:   Rgba = rgb(24, 24, 27);   // sidebar bg (#18181b)
pub const BG_TITLEBAR:  Rgba = rgb(9, 9, 11);     // titlebar bg (#09090b)
pub const BORDER:       Rgba = rgb(31, 31, 35);   // separator lines (#1f1f23)
pub const TEXT_PRIMARY: Rgba = rgb(250, 250, 250);// foreground text (#fafafa)
pub const TEXT_MUTED:   Rgba = rgb(161, 161, 170);// secondary text (#a1a1aa)
pub const TEXT_DIMMEST: Rgba = rgb(82, 82, 91);   // tertiary / section labels (#52525b)

// ── Workspace color chips (5 user-selectable accents) ─────────────────────
// Used for the color chip in titlebar + workspace switcher.
pub const CHIP_PURPLE: Rgba = BRAND;
pub const CHIP_BLUE:   Rgba = rgb(59, 130, 246);
pub const CHIP_GREEN:  Rgba = rgb(34, 197, 94);
pub const CHIP_AMBER:  Rgba = rgb(251, 191, 36);
pub const CHIP_PINK:   Rgba = rgb(236, 72, 153);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brand_purple_matches_cli_palette() {
        // rupu-cli/src/output/palette.rs::BRAND = Rgb(124, 58, 237).
        // Cross-surface coherence requires these stay in sync.
        assert_eq!(BRAND.r, 124.0 / 255.0);
        assert_eq!(BRAND.g, 58.0 / 255.0);
        assert_eq!(BRAND.b, 237.0 / 255.0);
        assert_eq!(BRAND.a, 1.0);
    }

    #[test]
    fn all_colors_are_opaque() {
        for color in [RUNNING, COMPLETE, FAILED, AWAITING, SKIPPED, DIM, BRAND] {
            assert_eq!(color.a, 1.0, "all palette colors should be fully opaque");
        }
    }
}
```

- [ ] **Step 2: Wire the module into main.rs**

Edit `crates/rupu-app/src/main.rs`, add the module declaration above `fn main()`:

```rust
mod palette;
```

- [ ] **Step 3: Run the tests**

Run: `cargo test -p rupu-app palette`
Expected: 2 tests pass.

- [ ] **Step 4: Commit**

```bash
git add crates/rupu-app/src/palette.rs crates/rupu-app/src/main.rs
git commit -m "feat(rupu-app): palette module lifted from rupu-cli/output/palette.rs"
```

---

## Task 4: WorkspaceManifest types

**Files:**
- Create: `crates/rupu-app/src/workspace/mod.rs`
- Create: `crates/rupu-app/src/workspace/manifest.rs`
- Modify: `crates/rupu-app/src/main.rs` (add `mod workspace;`)

- [ ] **Step 1: Write the failing test (round-trip toml)**

```rust
// crates/rupu-app/src/workspace/manifest.rs (test only — implementation comes next)
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_roundtrips_through_toml() {
        let m = WorkspaceManifest {
            id: "ws_01H8X123".into(),
            name: "rupu".into(),
            color: WorkspaceColor::Purple,
            path: "/Users/matt/Code/Oracle/rupu".into(),
            opened_at: chrono::DateTime::parse_from_rfc3339("2026-05-11T15:00:00Z")
                .unwrap()
                .with_timezone(&chrono::Utc),
            repos: vec![RepoBinding { r#ref: "github:Section9Labs/rupu".into() }],
            attached_hosts: vec![AttachedHost::Local],
            ui: UiState::default(),
        };
        let s = toml::to_string(&m).expect("serialize");
        let parsed: WorkspaceManifest = toml::from_str(&s).expect("deserialize");
        assert_eq!(parsed, m);
    }

    #[test]
    fn manifest_omits_empty_ui_fields() {
        // Default UiState should not pollute the toml with empty arrays.
        let m = WorkspaceManifest {
            id: "ws_01H8X123".into(),
            name: "minimal".into(),
            color: WorkspaceColor::Blue,
            path: "/tmp/x".into(),
            opened_at: chrono::Utc::now(),
            repos: vec![],
            attached_hosts: vec![AttachedHost::Local],
            ui: UiState::default(),
        };
        let s = toml::to_string(&m).expect("serialize");
        assert!(!s.contains("last_open_tabs"), "empty tabs should be omitted: {s}");
        assert!(!s.contains("sidebar_collapsed_sections"), "empty collapsed should be omitted: {s}");
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rupu-app manifest_roundtrips`
Expected: FAIL with "cannot find struct `WorkspaceManifest`".

- [ ] **Step 3: Implement the types**

Create the full `crates/rupu-app/src/workspace/manifest.rs`:

```rust
//! Per-user workspace manifest. Persists to
//! `~/Library/Application Support/rupu.app/workspaces/<id>.toml`
//! (path resolution lives in `storage.rs`).
//!
//! The split between manifest (per-user) and project-dir `.rupu/`
//! (per-project) is deliberate: the project directory stays clean
//! and shareable; per-user UI state survives `git clean -fdx`.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Top-level workspace manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceManifest {
    /// ULID prefixed with `ws_` (matches existing `run_*` convention).
    pub id: String,
    /// User-chosen display name (defaults to directory's basename on create).
    pub name: String,
    /// User-chosen accent color (sidebar chip + titlebar chip).
    pub color: WorkspaceColor,
    /// Absolute path to the workspace directory.
    pub path: String,
    /// Last time the workspace was opened — drives "Open Recent" ordering.
    pub opened_at: DateTime<Utc>,
    /// Repo refs attached to this workspace (rendered in sidebar `repos`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub repos: Vec<RepoBinding>,
    /// rupu execution hosts. Always at least one (Local). v2 adds Mcp.
    #[serde(default = "default_attached_hosts")]
    pub attached_hosts: Vec<AttachedHost>,
    /// Persisted UI state (open tabs, collapsed sidebar sections, etc.).
    #[serde(default)]
    pub ui: UiState,
}

fn default_attached_hosts() -> Vec<AttachedHost> {
    vec![AttachedHost::Local]
}

/// One of five preset accent colors. Stored as a lowercase string so
/// the toml stays human-editable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkspaceColor {
    Purple,
    Blue,
    Green,
    Amber,
    Pink,
}

impl WorkspaceColor {
    pub fn to_rgba(self) -> gpui::Rgba {
        use crate::palette::*;
        match self {
            Self::Purple => CHIP_PURPLE,
            Self::Blue => CHIP_BLUE,
            Self::Green => CHIP_GREEN,
            Self::Amber => CHIP_AMBER,
            Self::Pink => CHIP_PINK,
        }
    }
}

/// One repo attached to the workspace. The string format mirrors
/// rupu-scm's `RepoRef::parse` input: `<platform>:<owner>/<repo>`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoBinding {
    pub r#ref: String,
}

/// rupu execution host. v1 only supports Local (in-process orchestrator).
/// v2 will add `Mcp { url, auth_key }`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum AttachedHost {
    Local,
    // v2:
    // Mcp { url: String, auth_key: String },
}

/// UI state persisted per workspace. Empty by default; populated as
/// later sub-slices add tabs / view picker / etc.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiState {
    /// Tab identifiers in left-to-right order. Format: `<kind>:<id>` —
    /// `workflow:review`, `file:src/lib.rs`, `run:run_01ABC...`, etc.
    /// Resolved by later sub-slices; for D-1 this is always empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub last_open_tabs: Vec<String>,
    /// Sidebar sections the user has collapsed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sidebar_collapsed_sections: Vec<String>,
    /// Active view per tab — populated by D-2+ as views land.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub active_view_per_tab: BTreeMap<String, String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_roundtrips_through_toml() {
        let m = WorkspaceManifest {
            id: "ws_01H8X123".into(),
            name: "rupu".into(),
            color: WorkspaceColor::Purple,
            path: "/Users/matt/Code/Oracle/rupu".into(),
            opened_at: chrono::DateTime::parse_from_rfc3339("2026-05-11T15:00:00Z")
                .unwrap()
                .with_timezone(&chrono::Utc),
            repos: vec![RepoBinding { r#ref: "github:Section9Labs/rupu".into() }],
            attached_hosts: vec![AttachedHost::Local],
            ui: UiState::default(),
        };
        let s = toml::to_string(&m).expect("serialize");
        let parsed: WorkspaceManifest = toml::from_str(&s).expect("deserialize");
        assert_eq!(parsed, m);
    }

    #[test]
    fn manifest_omits_empty_ui_fields() {
        let m = WorkspaceManifest {
            id: "ws_01H8X123".into(),
            name: "minimal".into(),
            color: WorkspaceColor::Blue,
            path: "/tmp/x".into(),
            opened_at: chrono::Utc::now(),
            repos: vec![],
            attached_hosts: vec![AttachedHost::Local],
            ui: UiState::default(),
        };
        let s = toml::to_string(&m).expect("serialize");
        assert!(!s.contains("last_open_tabs"), "empty tabs should be omitted: {s}");
        assert!(!s.contains("sidebar_collapsed_sections"), "empty collapsed should be omitted: {s}");
        assert!(!s.contains("repos"), "empty repos should be omitted: {s}");
    }
}
```

- [ ] **Step 4: Create `workspace/mod.rs` (re-exports only for now)**

```rust
// crates/rupu-app/src/workspace/mod.rs
//! Workspace data layer — pure Rust, no GPUI.

pub mod manifest;
// storage, discovery, recents added in later tasks.

pub use manifest::{AttachedHost, RepoBinding, UiState, WorkspaceColor, WorkspaceManifest};
```

- [ ] **Step 5: Wire into main.rs**

```rust
mod palette;
mod workspace;
```

- [ ] **Step 6: Run tests, verify they pass**

Run: `cargo test -p rupu-app manifest`
Expected: 2 tests pass.

- [ ] **Step 7: Commit**

```bash
git add crates/rupu-app/src/workspace crates/rupu-app/src/main.rs
git commit -m "feat(rupu-app): WorkspaceManifest type + serde round-trip tests"
```

---

## Task 5: Storage path resolution

**Files:**
- Create: `crates/rupu-app/src/workspace/storage.rs`
- Modify: `crates/rupu-app/src/workspace/mod.rs` (re-export `storage`)

- [ ] **Step 1: Write the failing test**

```rust
// crates/rupu-app/src/workspace/storage.rs (test-only, impl comes next)
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspaces_dir_is_under_app_support() {
        let dir = workspaces_dir().expect("xdg lookup");
        let s = dir.to_string_lossy();
        assert!(s.contains("rupu.app"), "{s} should contain 'rupu.app'");
        assert!(s.contains("workspaces"), "{s} should contain 'workspaces'");
    }

    #[test]
    fn manifest_path_uses_id() {
        let p = manifest_path("ws_01H8X").expect("xdg lookup");
        assert!(p.ends_with("ws_01H8X.toml"), "manifest path should be <id>.toml: {p:?}");
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rupu-app storage`
Expected: FAIL with "cannot find function `workspaces_dir`".

- [ ] **Step 3: Implement**

```rust
// crates/rupu-app/src/workspace/storage.rs
//! Paths + manifest load/save for workspaces.
//!
//! macOS layout (via `directories::ProjectDirs`):
//!     ~/Library/Application Support/rupu.app/workspaces/<id>.toml
//!
//! The `ProjectDirs` qualifier triplet ("dev", "rupu", "rupu.app")
//! yields `rupu.app` as the leaf directory; this matches what every
//! macOS app bundle would create natively.

use crate::workspace::manifest::WorkspaceManifest;
use anyhow::{Context, Result};
use directories::ProjectDirs;
use std::path::PathBuf;

/// `~/Library/Application Support/rupu.app/workspaces/`. Creates the
/// directory on first call so callers can write to it unconditionally.
pub fn workspaces_dir() -> Result<PathBuf> {
    let proj = ProjectDirs::from("dev", "rupu", "rupu.app")
        .context("could not resolve user app-support dir for rupu.app")?;
    let dir = proj.config_dir().join("workspaces");
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("create {}", dir.display()))?;
    Ok(dir)
}

/// Path to a specific workspace's manifest file.
pub fn manifest_path(workspace_id: &str) -> Result<PathBuf> {
    Ok(workspaces_dir()?.join(format!("{workspace_id}.toml")))
}

/// Load a manifest from disk. Missing file → error.
pub fn load(workspace_id: &str) -> Result<WorkspaceManifest> {
    let path = manifest_path(workspace_id)?;
    let bytes = std::fs::read_to_string(&path)
        .with_context(|| format!("read {}", path.display()))?;
    toml::from_str(&bytes)
        .with_context(|| format!("parse {}", path.display()))
}

/// Save a manifest to disk. Overwrites any existing file at the same
/// path. Atomic write: serialize to a tempfile then rename.
pub fn save(m: &WorkspaceManifest) -> Result<()> {
    let path = manifest_path(&m.id)?;
    let tmp = path.with_extension("toml.tmp");
    let body = toml::to_string(m).context("serialize manifest")?;
    std::fs::write(&tmp, body).with_context(|| format!("write {}", tmp.display()))?;
    std::fs::rename(&tmp, &path).with_context(|| format!("rename {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspaces_dir_is_under_app_support() {
        let dir = workspaces_dir().expect("xdg lookup");
        let s = dir.to_string_lossy();
        assert!(s.contains("rupu.app"), "{s} should contain 'rupu.app'");
        assert!(s.contains("workspaces"), "{s} should contain 'workspaces'");
    }

    #[test]
    fn manifest_path_uses_id() {
        let p = manifest_path("ws_01H8X").expect("xdg lookup");
        assert!(p.ends_with("ws_01H8X.toml"), "manifest path should be <id>.toml: {p:?}");
    }
}
```

- [ ] **Step 4: Re-export from `workspace/mod.rs`**

```rust
pub mod manifest;
pub mod storage;

pub use manifest::{AttachedHost, RepoBinding, UiState, WorkspaceColor, WorkspaceManifest};
```

- [ ] **Step 5: Run tests, verify they pass**

Run: `cargo test -p rupu-app storage`
Expected: 2 tests pass.

- [ ] **Step 6: Commit**

```bash
git add crates/rupu-app/src/workspace
git commit -m "feat(rupu-app): workspace storage paths + atomic manifest load/save"
```

---

## Task 6: Manifest round-trip integration test

**Files:**
- Create: `crates/rupu-app/tests/workspace_manifest_roundtrip.rs`

- [ ] **Step 1: Write the integration test**

```rust
// crates/rupu-app/tests/workspace_manifest_roundtrip.rs
//! End-to-end test: build a WorkspaceManifest, save it via storage::save,
//! load it back via storage::load, assert equality. Uses a TempDir +
//! XDG_CONFIG_HOME override so the test doesn't touch the real
//! ~/Library/Application Support tree.

use rupu_app::workspace::{
    manifest::{AttachedHost, RepoBinding, UiState, WorkspaceColor, WorkspaceManifest},
    storage,
};
use std::env;
use tempfile::TempDir;

#[test]
fn save_then_load_yields_identical_manifest() {
    // Sandbox: directories crate honors XDG_CONFIG_HOME on Linux but
    // NOT macOS — there we have to redirect HOME instead. We do both
    // so this test passes on both targets.
    let tmp = TempDir::new().expect("tempdir");
    env::set_var("HOME", tmp.path());
    env::set_var("XDG_CONFIG_HOME", tmp.path().join(".config"));

    let original = WorkspaceManifest {
        id: format!("ws_{}", ulid::Ulid::new()),
        name: "test-workspace".into(),
        color: WorkspaceColor::Pink,
        path: "/tmp/test-project".into(),
        opened_at: chrono::Utc::now(),
        repos: vec![
            RepoBinding { r#ref: "github:acme/foo".into() },
            RepoBinding { r#ref: "gitlab:acme/bar".into() },
        ],
        attached_hosts: vec![AttachedHost::Local],
        ui: UiState::default(),
    };

    storage::save(&original).expect("save");
    let loaded = storage::load(&original.id).expect("load");

    assert_eq!(loaded, original);
}
```

- [ ] **Step 2: Add `tempfile` to dev-dependencies**

In `crates/rupu-app/Cargo.toml`:

```toml
[dev-dependencies]
tempfile.workspace = true
```

(`tempfile` is already in workspace deps — used by `rupu-orchestrator` tests.)

- [ ] **Step 3: Run the test**

Run: `cargo test -p rupu-app --test workspace_manifest_roundtrip`
Expected: 1 test passes.

- [ ] **Step 4: Commit**

```bash
git add crates/rupu-app/Cargo.toml crates/rupu-app/tests/workspace_manifest_roundtrip.rs
git commit -m "test(rupu-app): integration test for manifest save/load round-trip"
```

---

## Task 7: Asset discovery

**Files:**
- Create: `crates/rupu-app/src/workspace/discovery.rs`
- Modify: `crates/rupu-app/src/workspace/mod.rs`

- [ ] **Step 1: Write the failing test**

```rust
// crates/rupu-app/src/workspace/discovery.rs (test only — impl comes next)
#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn discover_finds_workflows_agents_autoflows() {
        let tmp = TempDir::new().unwrap();
        let rupu = tmp.path().join(".rupu");
        fs::create_dir_all(rupu.join("workflows")).unwrap();
        fs::create_dir_all(rupu.join("agents")).unwrap();
        fs::create_dir_all(rupu.join("autoflows")).unwrap();
        fs::write(rupu.join("workflows/review.yaml"), "name: review").unwrap();
        fs::write(rupu.join("workflows/dispatch.yaml"), "name: dispatch").unwrap();
        fs::write(rupu.join("agents/sec.md"), "---\nname: sec\n---").unwrap();
        fs::write(rupu.join("autoflows/nightly.yaml"), "name: nightly").unwrap();

        let assets = discover_project(tmp.path());

        assert_eq!(assets.workflows.len(), 2);
        assert!(assets.workflows.iter().any(|a| a.name == "review"));
        assert!(assets.workflows.iter().any(|a| a.name == "dispatch"));
        assert_eq!(assets.agents.len(), 1);
        assert_eq!(assets.agents[0].name, "sec");
        assert_eq!(assets.autoflows.len(), 1);
        assert_eq!(assets.autoflows[0].name, "nightly");
    }

    #[test]
    fn discover_missing_dir_returns_empty() {
        let tmp = TempDir::new().unwrap();
        // No .rupu/ at all.
        let assets = discover_project(tmp.path());
        assert!(assets.workflows.is_empty());
        assert!(assets.agents.is_empty());
        assert!(assets.autoflows.is_empty());
    }

    #[test]
    fn discover_ignores_wrong_extensions() {
        let tmp = TempDir::new().unwrap();
        let rupu = tmp.path().join(".rupu");
        fs::create_dir_all(rupu.join("workflows")).unwrap();
        fs::write(rupu.join("workflows/review.yaml"), "").unwrap();
        fs::write(rupu.join("workflows/README.md"), "").unwrap();  // not a workflow
        fs::write(rupu.join("workflows/.DS_Store"), "").unwrap(); // junk

        let assets = discover_project(tmp.path());
        assert_eq!(assets.workflows.len(), 1);
        assert_eq!(assets.workflows[0].name, "review");
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rupu-app discovery`
Expected: FAIL with "cannot find function `discover_project`".

- [ ] **Step 3: Implement**

```rust
// crates/rupu-app/src/workspace/discovery.rs
//! Walk a project directory's `.rupu/{workflows,agents,autoflows}/`
//! (and the global `~/.rupu/...`) to populate the sidebar.
//!
//! D-1 scope: just filenames + paths. No YAML parsing; later
//! sub-slices (D-2 Graph view, D-7 Agent editor) parse contents on
//! demand when a file is opened. This keeps workspace open fast even
//! for projects with hundreds of agents.

use std::path::{Path, PathBuf};

/// One discovered asset (workflow / agent / autoflow file).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asset {
    /// Filename without extension (e.g. `review.yaml` → `"review"`).
    pub name: String,
    /// Absolute path on disk.
    pub path: PathBuf,
}

/// All assets discovered for one location (project dir or global dir).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AssetSet {
    pub workflows: Vec<Asset>,
    pub agents: Vec<Asset>,
    pub autoflows: Vec<Asset>,
}

/// Walk `<project_dir>/.rupu/{workflows,agents,autoflows}/`. Each is
/// optional — missing dirs yield empty vecs. Files are sorted by
/// name for deterministic UI ordering.
pub fn discover_project(project_dir: &Path) -> AssetSet {
    let rupu = project_dir.join(".rupu");
    AssetSet {
        workflows: list(&rupu.join("workflows"), "yaml"),
        agents:    list(&rupu.join("agents"), "md"),
        autoflows: list(&rupu.join("autoflows"), "yaml"),
    }
}

/// Walk `~/.rupu/{workflows,agents,autoflows}/`. Returns empty if the
/// HOME directory can't be resolved or the dirs are absent.
pub fn discover_global() -> AssetSet {
    let Some(home) = dirs_home() else {
        return AssetSet::default();
    };
    let rupu = home.join(".rupu");
    AssetSet {
        workflows: list(&rupu.join("workflows"), "yaml"),
        agents:    list(&rupu.join("agents"), "md"),
        autoflows: list(&rupu.join("autoflows"), "yaml"),
    }
}

fn dirs_home() -> Option<PathBuf> {
    directories::BaseDirs::new().map(|d| d.home_dir().to_path_buf())
}

/// List one directory, filtering to a single extension. Sorted by
/// filename.
fn list(dir: &Path, ext: &str) -> Vec<Asset> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<Asset> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some(ext))
        .filter_map(|p| {
            let name = p.file_stem()?.to_str()?.to_string();
            Some(Asset { name, path: p })
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn discover_finds_workflows_agents_autoflows() {
        let tmp = TempDir::new().unwrap();
        let rupu = tmp.path().join(".rupu");
        fs::create_dir_all(rupu.join("workflows")).unwrap();
        fs::create_dir_all(rupu.join("agents")).unwrap();
        fs::create_dir_all(rupu.join("autoflows")).unwrap();
        fs::write(rupu.join("workflows/review.yaml"), "name: review").unwrap();
        fs::write(rupu.join("workflows/dispatch.yaml"), "name: dispatch").unwrap();
        fs::write(rupu.join("agents/sec.md"), "---\nname: sec\n---").unwrap();
        fs::write(rupu.join("autoflows/nightly.yaml"), "name: nightly").unwrap();

        let assets = discover_project(tmp.path());

        assert_eq!(assets.workflows.len(), 2);
        assert!(assets.workflows.iter().any(|a| a.name == "review"));
        assert!(assets.workflows.iter().any(|a| a.name == "dispatch"));
        assert_eq!(assets.agents.len(), 1);
        assert_eq!(assets.agents[0].name, "sec");
        assert_eq!(assets.autoflows.len(), 1);
        assert_eq!(assets.autoflows[0].name, "nightly");
    }

    #[test]
    fn discover_missing_dir_returns_empty() {
        let tmp = TempDir::new().unwrap();
        let assets = discover_project(tmp.path());
        assert!(assets.workflows.is_empty());
        assert!(assets.agents.is_empty());
        assert!(assets.autoflows.is_empty());
    }

    #[test]
    fn discover_ignores_wrong_extensions() {
        let tmp = TempDir::new().unwrap();
        let rupu = tmp.path().join(".rupu");
        fs::create_dir_all(rupu.join("workflows")).unwrap();
        fs::write(rupu.join("workflows/review.yaml"), "").unwrap();
        fs::write(rupu.join("workflows/README.md"), "").unwrap();
        fs::write(rupu.join("workflows/.DS_Store"), "").unwrap();

        let assets = discover_project(tmp.path());
        assert_eq!(assets.workflows.len(), 1);
        assert_eq!(assets.workflows[0].name, "review");
    }
}
```

- [ ] **Step 4: Re-export from `mod.rs`**

```rust
pub mod discovery;
pub mod manifest;
pub mod storage;

pub use discovery::{Asset, AssetSet};
pub use manifest::{AttachedHost, RepoBinding, UiState, WorkspaceColor, WorkspaceManifest};
```

- [ ] **Step 5: Run tests, verify they pass**

Run: `cargo test -p rupu-app discovery`
Expected: 3 tests pass.

- [ ] **Step 6: Commit**

```bash
git add crates/rupu-app/src/workspace
git commit -m "feat(rupu-app): asset discovery for project + global .rupu/ dirs"
```

---

## Task 8: Recent workspaces

**Files:**
- Create: `crates/rupu-app/src/workspace/recents.rs`
- Modify: `crates/rupu-app/src/workspace/mod.rs`

- [ ] **Step 1: Write the failing test**

```rust
// crates/rupu-app/src/workspace/recents.rs (test only — impl comes next)
#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::manifest::{AttachedHost, UiState, WorkspaceColor, WorkspaceManifest};
    use crate::workspace::storage;
    use chrono::TimeZone;
    use std::env;
    use tempfile::TempDir;

    fn sandbox() -> TempDir {
        let tmp = TempDir::new().unwrap();
        env::set_var("HOME", tmp.path());
        env::set_var("XDG_CONFIG_HOME", tmp.path().join(".config"));
        tmp
    }

    fn mk(id: &str, opened_at_secs: i64) -> WorkspaceManifest {
        WorkspaceManifest {
            id: id.into(),
            name: id.into(),
            color: WorkspaceColor::Purple,
            path: format!("/tmp/{id}"),
            opened_at: chrono::Utc.timestamp_opt(opened_at_secs, 0).unwrap(),
            repos: vec![],
            attached_hosts: vec![AttachedHost::Local],
            ui: UiState::default(),
        }
    }

    #[test]
    fn list_returns_newest_first() {
        let _tmp = sandbox();
        storage::save(&mk("ws_a", 1_000)).unwrap();
        storage::save(&mk("ws_b", 3_000)).unwrap();
        storage::save(&mk("ws_c", 2_000)).unwrap();

        let recents = list().expect("list");
        assert_eq!(recents.len(), 3);
        assert_eq!(recents[0].id, "ws_b"); // 3000 — newest
        assert_eq!(recents[1].id, "ws_c"); // 2000
        assert_eq!(recents[2].id, "ws_a"); // 1000
    }

    #[test]
    fn list_skips_non_toml_files() {
        let _tmp = sandbox();
        storage::save(&mk("ws_real", 1)).unwrap();
        let dir = storage::workspaces_dir().unwrap();
        std::fs::write(dir.join("garbage.txt"), "not a manifest").unwrap();
        std::fs::write(dir.join(".DS_Store"), "macos junk").unwrap();

        let recents = list().expect("list");
        assert_eq!(recents.len(), 1);
        assert_eq!(recents[0].id, "ws_real");
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rupu-app recents`
Expected: FAIL with "cannot find function `list`".

- [ ] **Step 3: Implement**

```rust
// crates/rupu-app/src/workspace/recents.rs
//! Recent-workspaces listing.
//!
//! Walks `storage::workspaces_dir()`, parses each `*.toml`, sorts by
//! `opened_at` descending. Errors on individual files are logged and
//! skipped — a corrupt manifest shouldn't block the list of valid ones.

use crate::workspace::{manifest::WorkspaceManifest, storage};
use anyhow::Result;
use std::cmp::Reverse;

/// All workspaces with valid manifests, newest-opened first.
pub fn list() -> Result<Vec<WorkspaceManifest>> {
    let dir = storage::workspaces_dir()?;
    let entries = match std::fs::read_dir(&dir) {
        Ok(rd) => rd,
        Err(_) => return Ok(Vec::new()),
    };

    let mut out: Vec<WorkspaceManifest> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("toml"))
        .filter_map(|p| {
            let bytes = std::fs::read_to_string(&p).ok()?;
            match toml::from_str::<WorkspaceManifest>(&bytes) {
                Ok(m) => Some(m),
                Err(e) => {
                    tracing::warn!(path = %p.display(), error = %e, "skip unreadable manifest");
                    None
                }
            }
        })
        .collect();

    out.sort_by_key(|m| Reverse(m.opened_at));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::manifest::{AttachedHost, UiState, WorkspaceColor, WorkspaceManifest};
    use chrono::TimeZone;
    use std::env;
    use tempfile::TempDir;

    fn sandbox() -> TempDir {
        let tmp = TempDir::new().unwrap();
        env::set_var("HOME", tmp.path());
        env::set_var("XDG_CONFIG_HOME", tmp.path().join(".config"));
        tmp
    }

    fn mk(id: &str, opened_at_secs: i64) -> WorkspaceManifest {
        WorkspaceManifest {
            id: id.into(),
            name: id.into(),
            color: WorkspaceColor::Purple,
            path: format!("/tmp/{id}"),
            opened_at: chrono::Utc.timestamp_opt(opened_at_secs, 0).unwrap(),
            repos: vec![],
            attached_hosts: vec![AttachedHost::Local],
            ui: UiState::default(),
        }
    }

    #[test]
    fn list_returns_newest_first() {
        let _tmp = sandbox();
        storage::save(&mk("ws_a", 1_000)).unwrap();
        storage::save(&mk("ws_b", 3_000)).unwrap();
        storage::save(&mk("ws_c", 2_000)).unwrap();

        let recents = list().expect("list");
        assert_eq!(recents.len(), 3);
        assert_eq!(recents[0].id, "ws_b");
        assert_eq!(recents[1].id, "ws_c");
        assert_eq!(recents[2].id, "ws_a");
    }

    #[test]
    fn list_skips_non_toml_files() {
        let _tmp = sandbox();
        storage::save(&mk("ws_real", 1)).unwrap();
        let dir = storage::workspaces_dir().unwrap();
        std::fs::write(dir.join("garbage.txt"), "not a manifest").unwrap();
        std::fs::write(dir.join(".DS_Store"), "macos junk").unwrap();

        let recents = list().expect("list");
        assert_eq!(recents.len(), 1);
        assert_eq!(recents[0].id, "ws_real");
    }
}
```

- [ ] **Step 4: Re-export from `mod.rs`**

```rust
pub mod discovery;
pub mod manifest;
pub mod recents;
pub mod storage;

pub use discovery::{Asset, AssetSet};
pub use manifest::{AttachedHost, RepoBinding, UiState, WorkspaceColor, WorkspaceManifest};
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p rupu-app recents`
Expected: 2 tests pass.

- [ ] **Step 6: Commit**

```bash
git add crates/rupu-app/src/workspace
git commit -m "feat(rupu-app): recents.rs — list workspaces newest-first"
```

---

## Task 9: Workspace handle struct

**Files:**
- Create: combine into `crates/rupu-app/src/workspace/handle.rs` (new file)
- Modify: `crates/rupu-app/src/workspace/mod.rs`

A `Workspace` handle bundles a manifest with its discovered assets — the GPUI window layers consume this single struct rather than reaching into each data module directly. Construction touches the filesystem (discovery walks), so it's a fallible builder rather than `Default`.

- [ ] **Step 1: Write the failing test**

```rust
// crates/rupu-app/src/workspace/handle.rs (test only — impl comes next)
#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use tempfile::TempDir;

    fn sandbox() -> TempDir {
        let tmp = TempDir::new().unwrap();
        env::set_var("HOME", tmp.path());
        env::set_var("XDG_CONFIG_HOME", tmp.path().join(".config"));
        tmp
    }

    #[test]
    fn open_directory_creates_manifest_on_first_open() {
        let _home = sandbox();
        let project = TempDir::new().unwrap();
        let ws = Workspace::open(project.path()).expect("open");

        assert!(ws.manifest.id.starts_with("ws_"));
        assert_eq!(ws.manifest.path, project.path().to_string_lossy().to_string());
        assert_eq!(ws.manifest.color, WorkspaceColor::Purple); // default
        // Default name = directory's basename.
        let basename = project.path().file_name().unwrap().to_str().unwrap();
        assert_eq!(ws.manifest.name, basename);

        // Manifest was persisted.
        let loaded = storage::load(&ws.manifest.id).expect("load");
        assert_eq!(loaded.id, ws.manifest.id);
    }

    #[test]
    fn reopen_reuses_existing_manifest() {
        let _home = sandbox();
        let project = TempDir::new().unwrap();

        let first = Workspace::open(project.path()).expect("first open");
        let second = Workspace::open(project.path()).expect("second open");

        assert_eq!(first.manifest.id, second.manifest.id);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rupu-app workspace::handle`
Expected: FAIL with "cannot find struct `Workspace`".

- [ ] **Step 3: Implement**

```rust
// crates/rupu-app/src/workspace/handle.rs
//! `Workspace` handle — the runtime object the GPUI window layers
//! consume. Wraps a `WorkspaceManifest` with its discovered project
//! + global asset sets. Constructing a handle is a fallible IO
//! operation (manifest load/create + asset walk); the GPUI window
//! constructor calls `open` and bails on error.

use crate::workspace::{
    discovery::{self, AssetSet},
    manifest::{AttachedHost, UiState, WorkspaceColor, WorkspaceManifest},
    recents, storage,
};
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct Workspace {
    pub manifest: WorkspaceManifest,
    pub project_assets: AssetSet,
    pub global_assets: AssetSet,
}

impl Workspace {
    /// Open a workspace for a given project directory. If a manifest
    /// already exists for this path (in `recents()`), reuse its id and
    /// settings; otherwise create a fresh manifest with default name
    /// (directory basename) and default color (Purple).
    ///
    /// Either way the `opened_at` timestamp is bumped to `now()` and
    /// the manifest is persisted before returning.
    pub fn open(project_dir: &Path) -> Result<Self> {
        let absolute = project_dir
            .canonicalize()
            .with_context(|| format!("canonicalize {}", project_dir.display()))?;

        let mut manifest = match find_existing(&absolute)? {
            Some(m) => m,
            None => fresh_manifest(&absolute),
        };
        manifest.opened_at = chrono::Utc::now();
        storage::save(&manifest).context("persist manifest on open")?;

        let project_assets = discovery::discover_project(&absolute);
        let global_assets = discovery::discover_global();

        Ok(Workspace {
            manifest,
            project_assets,
            global_assets,
        })
    }
}

/// Look through the recents list for a manifest whose path canonicalizes
/// to the requested directory. Returns the first match, if any.
fn find_existing(absolute_path: &Path) -> Result<Option<WorkspaceManifest>> {
    let want = absolute_path.to_string_lossy();
    for m in recents::list()? {
        if PathBuf::from(&m.path)
            .canonicalize()
            .map(|p| p.to_string_lossy() == want)
            .unwrap_or(false)
        {
            return Ok(Some(m));
        }
    }
    Ok(None)
}

/// Build a default-shaped manifest for a previously-unseen directory.
fn fresh_manifest(absolute_path: &Path) -> WorkspaceManifest {
    let name = absolute_path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("workspace")
        .to_string();
    WorkspaceManifest {
        id: format!("ws_{}", ulid::Ulid::new()),
        name,
        color: WorkspaceColor::Purple,
        path: absolute_path.to_string_lossy().to_string(),
        opened_at: chrono::Utc::now(),
        repos: vec![],
        attached_hosts: vec![AttachedHost::Local],
        ui: UiState::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use tempfile::TempDir;

    fn sandbox() -> TempDir {
        let tmp = TempDir::new().unwrap();
        env::set_var("HOME", tmp.path());
        env::set_var("XDG_CONFIG_HOME", tmp.path().join(".config"));
        tmp
    }

    #[test]
    fn open_directory_creates_manifest_on_first_open() {
        let _home = sandbox();
        let project = TempDir::new().unwrap();
        let ws = Workspace::open(project.path()).expect("open");

        assert!(ws.manifest.id.starts_with("ws_"));
        assert_eq!(
            ws.manifest.path,
            project.path().canonicalize().unwrap().to_string_lossy().to_string()
        );
        assert_eq!(ws.manifest.color, WorkspaceColor::Purple);
        let basename = project
            .path()
            .canonicalize()
            .unwrap()
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        assert_eq!(ws.manifest.name, basename);

        let loaded = storage::load(&ws.manifest.id).expect("load");
        assert_eq!(loaded.id, ws.manifest.id);
    }

    #[test]
    fn reopen_reuses_existing_manifest() {
        let _home = sandbox();
        let project = TempDir::new().unwrap();

        let first = Workspace::open(project.path()).expect("first open");
        let second = Workspace::open(project.path()).expect("second open");

        assert_eq!(first.manifest.id, second.manifest.id);
    }
}
```

- [ ] **Step 4: Re-export from `mod.rs`**

```rust
pub mod discovery;
pub mod handle;
pub mod manifest;
pub mod recents;
pub mod storage;

pub use discovery::{Asset, AssetSet};
pub use handle::Workspace;
pub use manifest::{AttachedHost, RepoBinding, UiState, WorkspaceColor, WorkspaceManifest};
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p rupu-app workspace::handle`
Expected: 2 tests pass.

- [ ] **Step 6: Commit**

```bash
git add crates/rupu-app/src/workspace
git commit -m "feat(rupu-app): Workspace handle — open() builds manifest + assets"
```

---

## Task 10: GPUI app boot

**Files:**
- Modify: `crates/rupu-app/src/main.rs`

Replace the placeholder `main()` with a real GPUI `App::new().run(...)` boot. No windows yet — that comes in Task 11. This task verifies the binary actually starts a GPUI app loop without crashing.

> **Note on GPUI's pre-1.0 API:** the exact method names below (e.g. `App::new`, `cx.activate`) match the public examples in `zed-industries/zed` at the pinned commit. If GPUI's API has drifted by the time you run this, consult `crates/gpui/examples/` in the pinned Zed source for the current entry-point pattern. The shape of the call (app handle → run closure → cx for window registration) is stable.

- [ ] **Step 1: Update `main.rs`**

```rust
//! rupu.app — native macOS desktop app.
//!
//! See `docs/superpowers/specs/2026-05-11-rupu-slice-d-app-design.md`.

mod palette;
mod workspace;

use gpui::App;

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "rupu_app=debug,gpui=info".into()),
        )
        .init();
    tracing::info!("rupu.app starting");

    App::new().run(|cx| {
        // Activate the app so it gets focus on launch.
        cx.activate(true);

        // No windows yet — they land in Task 11 once the
        // WorkspaceWindow view exists. For now we boot the app
        // loop so `cargo run -p rupu-app` proves the binary works.
        tracing::info!("rupu.app app-loop entered (no windows yet)");
    });
}
```

- [ ] **Step 2: Build it**

Run: `cargo build -p rupu-app`
Expected: builds. If gpui's API has changed, fix the imports per the note above.

- [ ] **Step 3: Smoke-run it briefly**

Run: `timeout 3 cargo run -p rupu-app || true`
Expected: prints `rupu.app starting` + `rupu.app app-loop entered (no windows yet)` on stderr (via tracing), then exits after 3s with no panic. (App with no windows exits on its own quickly; the timeout is a safety net.)

- [ ] **Step 4: Commit**

```bash
git add crates/rupu-app/src/main.rs
git commit -m "feat(rupu-app): boot GPUI app loop (no windows yet)"
```

---

## Task 11: Workspace window shell

**Files:**
- Create: `crates/rupu-app/src/window/mod.rs`
- Modify: `crates/rupu-app/src/main.rs` (add `mod window;`)

This task introduces the window itself. For D-1 the window has the right shape (titlebar + sidebar + main area placeholder) but the sidebar and titlebar are stub views — they get implemented in Tasks 12 and 13.

> **GPUI view pattern:** views in GPUI are types that implement `Render`. `App.open_window(opts, |cx| view)` constructs the view. The exact opts struct (`WindowOptions`) varies by GPUI version; the shape below matches the public examples.

- [ ] **Step 1: Create the window module**

```rust
// crates/rupu-app/src/window/mod.rs
//! WorkspaceWindow — the GPUI view for one workspace's window.

use crate::palette;
use crate::workspace::Workspace;
use gpui::{div, prelude::*, App, Bounds, IntoElement, Pixels, Render, Size, ViewContext, WindowBounds, WindowHandle, WindowOptions};

pub mod sidebar;
pub mod titlebar;

pub struct WorkspaceWindow {
    pub workspace: Workspace,
}

impl WorkspaceWindow {
    /// Open a new top-level window for the given workspace. The
    /// window owns the workspace handle for its lifetime.
    pub fn open(workspace: Workspace, cx: &mut App) -> WindowHandle<Self> {
        let opts = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                None,
                Size::new(Pixels(1240.0), Pixels(800.0)),
                cx,
            ))),
            titlebar: None, // we draw our own titlebar inside the view
            ..Default::default()
        };
        cx.open_window(opts, |_cx| WorkspaceWindow { workspace })
    }
}

impl Render for WorkspaceWindow {
    fn render(&mut self, _cx: &mut ViewContext<Self>) -> impl IntoElement {
        // Three regions, stacked vertically:
        //   [ titlebar ]
        //   [ sidebar | main-area-placeholder ]
        div()
            .size_full()
            .bg(palette::BG_PRIMARY)
            .text_color(palette::TEXT_PRIMARY)
            .flex()
            .flex_col()
            .child(titlebar::render(&self.workspace))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_1()
                    .child(sidebar::render(&self.workspace))
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_color(palette::TEXT_DIMMEST)
                            .child("Open a workflow from the sidebar."),
                    ),
            )
    }
}
```

- [ ] **Step 2: Stub the titlebar and sidebar modules**

Create `crates/rupu-app/src/window/titlebar.rs`:

```rust
//! Titlebar — stub for Task 12. Returns a placeholder.

use crate::palette;
use crate::workspace::Workspace;
use gpui::{div, prelude::*, IntoElement};

pub fn render(_workspace: &Workspace) -> impl IntoElement {
    div()
        .h(gpui::Pixels(36.0))
        .bg(palette::BG_TITLEBAR)
        .border_b_1()
        .border_color(palette::BORDER)
        .child("titlebar (stub)")
}
```

Create `crates/rupu-app/src/window/sidebar.rs`:

```rust
//! Sidebar — stub for Task 13. Returns a placeholder.

use crate::palette;
use crate::workspace::Workspace;
use gpui::{div, prelude::*, IntoElement, Pixels};

pub fn render(_workspace: &Workspace) -> impl IntoElement {
    div()
        .w(Pixels(180.0))
        .h_full()
        .bg(palette::BG_SIDEBAR)
        .border_r_1()
        .border_color(palette::BORDER)
        .child("sidebar (stub)")
}
```

- [ ] **Step 3: Wire `window` module into main.rs and open a fixture window on boot**

```rust
// crates/rupu-app/src/main.rs
mod palette;
mod window;
mod workspace;

use gpui::App;

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "rupu_app=debug,gpui=info".into()),
        )
        .init();

    // For D-1 development: open whichever directory the user passes
    // as the first CLI arg, or fall back to cwd. The proper "File >
    // Open Workspace…" picker lands in Task 15.
    let project_dir = std::env::args().nth(1)
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().expect("cwd"));

    let workspace = workspace::Workspace::open(&project_dir)
        .expect("open workspace");
    tracing::info!(id = %workspace.manifest.id, "opened workspace");

    App::new().run(move |cx| {
        cx.activate(true);
        window::WorkspaceWindow::open(workspace, cx);
    });
}
```

- [ ] **Step 4: Build + smoke run**

Run: `cargo build -p rupu-app`
Expected: builds.

Run: `timeout 5 cargo run -p rupu-app -- /Users/matt/Code/Oracle/rupu || true`
Expected: a window opens for ~5 seconds showing dark background, stub titlebar / sidebar / "Open a workflow…" placeholder. Closes cleanly when timeout fires (or manually by ⌘W). No panics in stderr.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-app/src/main.rs crates/rupu-app/src/window
git commit -m "feat(rupu-app): WorkspaceWindow shell — titlebar + sidebar + main placeholder"
```

---

## Task 12: Titlebar component

**Files:**
- Modify: `crates/rupu-app/src/window/titlebar.rs`

Replace the stub from Task 11 with the real titlebar: color chip (10px circle in the workspace's accent color) · workspace name (bold) · in-flight count badge (always renders `0` for D-1 since no executor exists yet — D-3 wires the live count).

- [ ] **Step 1: Implement the titlebar**

```rust
// crates/rupu-app/src/window/titlebar.rs
//! Titlebar: color chip · workspace name · in-flight count badge.
//!
//! Per spec §6.1, the count is this-workspace only (the system
//! menubar in `menu/menubar.rs` carries the cross-workspace
//! count). For D-1 the count is hard-wired to 0; D-3 lights it
//! up when the executor lands.

use crate::palette;
use crate::workspace::Workspace;
use gpui::{div, prelude::*, IntoElement, Pixels};

pub fn render(workspace: &Workspace) -> impl IntoElement {
    let chip_color = workspace.manifest.color.to_rgba();
    let in_flight = 0u32; // wired up in D-3

    div()
        .h(Pixels(36.0))
        .bg(palette::BG_TITLEBAR)
        .border_b_1()
        .border_color(palette::BORDER)
        .px(Pixels(14.0))
        .flex()
        .items_center()
        .gap(Pixels(10.0))
        .child(
            // 10px color chip
            div()
                .w(Pixels(10.0))
                .h(Pixels(10.0))
                .rounded_full()
                .bg(chip_color),
        )
        .child(
            div()
                .text_color(palette::TEXT_PRIMARY)
                .text_sm()
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .child(workspace.manifest.name.clone()),
        )
        .child(
            // In-flight count — only renders when > 0. D-1 always shows nothing.
            if in_flight > 0 {
                div()
                    .ml(Pixels(8.0))
                    .px(Pixels(6.0))
                    .py(Pixels(1.0))
                    .rounded(Pixels(4.0))
                    .bg(palette::RUNNING)
                    .text_color(palette::TEXT_PRIMARY)
                    .text_xs()
                    .child(format!("{in_flight} running"))
                    .into_any_element()
            } else {
                div().into_any_element()
            },
        )
}
```

- [ ] **Step 2: Build + smoke run**

Run: `cargo build -p rupu-app && timeout 5 cargo run -p rupu-app -- /Users/matt/Code/Oracle/rupu || true`
Expected: window opens with purple color chip + "rupu" name in titlebar; no count badge (since count is 0).

- [ ] **Step 3: Commit**

```bash
git add crates/rupu-app/src/window/titlebar.rs
git commit -m "feat(rupu-app): titlebar with color chip + name + (stub) in-flight count"
```

---

## Task 13: Sidebar accordion

**Files:**
- Modify: `crates/rupu-app/src/window/sidebar.rs`

Replace the stub with the real minimal accordion described in spec §6.2: tiny uppercase section labels, sections in fixed order (`workflows · runs · repos · agents · issues`), items inside each section. No interactivity yet (clicking items doesn't open tabs — that's D-2). The collapse/expand state lives in `UiState::sidebar_collapsed_sections` which we already persist.

For D-1 the `runs` and `issues` sections render as empty placeholders since runs come in D-3 and issues in D-9.

- [ ] **Step 1: Implement the sidebar**

```rust
// crates/rupu-app/src/window/sidebar.rs
//! Sidebar — minimal accordion per spec §6.2.
//!
//! Fixed section order: workflows · runs · repos · agents · issues.
//! Collapse state persists in `Workspace.manifest.ui.sidebar_collapsed_sections`.
//! For D-1, item clicks are no-ops (tabs land in D-2).

use crate::palette;
use crate::workspace::{Asset, Workspace};
use gpui::{div, prelude::*, IntoElement, Pixels};

const SIDEBAR_WIDTH: f32 = 180.0;
const SECTION_ORDER: &[&str] = &["workflows", "runs", "repos", "agents", "issues"];

pub fn render(workspace: &Workspace) -> impl IntoElement {
    let collapsed = &workspace.manifest.ui.sidebar_collapsed_sections;
    let project = &workspace.project_assets;
    let global = &workspace.global_assets;

    let mut container = div()
        .w(Pixels(SIDEBAR_WIDTH))
        .h_full()
        .bg(palette::BG_SIDEBAR)
        .border_r_1()
        .border_color(palette::BORDER)
        .px(Pixels(14.0))
        .py(Pixels(18.0))
        .flex()
        .flex_col();

    for (i, section) in SECTION_ORDER.iter().enumerate() {
        let is_collapsed = collapsed.iter().any(|s| s == section);
        let items: Vec<&Asset> = match *section {
            "workflows" => project.workflows.iter().chain(global.workflows.iter()).collect(),
            "agents"    => project.agents.iter().chain(global.agents.iter()).collect(),
            "repos"     => Vec::new(), // resolved from manifest.repos in D-9
            "runs"      => Vec::new(), // populated in D-3
            "issues"    => Vec::new(), // populated in D-9
            _ => Vec::new(),
        };
        container = container.child(render_section(section, &items, is_collapsed, i == 0));
    }

    container
}

fn render_section(name: &str, items: &[&Asset], collapsed: bool, is_first: bool) -> impl IntoElement {
    let header = div()
        .text_xs()
        .text_color(palette::TEXT_DIMMEST)
        .mb(Pixels(4.0))
        .when(!is_first, |d| d.mt(Pixels(18.0)))
        .flex()
        .items_center()
        .gap(Pixels(6.0))
        .child(div().child(name.to_string()))
        .when(collapsed, |d| {
            d.child(div().text_color(palette::TEXT_DIMMEST).child("▸"))
                .child(div().ml_auto().text_color(palette::TEXT_DIMMEST).child(format!("{}", items.len())))
        });

    let body = if collapsed {
        div() // collapsed: nothing
    } else if items.is_empty() {
        div().mt(Pixels(2.0)).text_xs().text_color(palette::TEXT_DIMMEST).child("—")
    } else {
        let mut list = div().flex().flex_col();
        for asset in items {
            list = list.child(
                div()
                    .text_xs()
                    .text_color(palette::TEXT_MUTED)
                    .child(asset.name.clone()),
            );
        }
        list
    };

    div().child(header).child(body)
}
```

> **Note:** the `.when()` combinator above is a common GPUI element-builder pattern; if your pinned version uses a different name (e.g. `.cond()` or imperative `if` blocks), adapt accordingly. The output shape (header row + conditional body) is the load-bearing part.

- [ ] **Step 2: Build + smoke run**

Run: `cargo build -p rupu-app && timeout 5 cargo run -p rupu-app -- /Users/matt/Code/Oracle/rupu || true`
Expected: window opens; sidebar shows 5 uppercase section labels (`workflows`, `runs`, `repos`, `agents`, `issues`); workflows section lists project workflows (review, dispatch-demo, etc. from `.rupu/workflows/`); agents section lists project agents; runs/repos/issues show `—`.

- [ ] **Step 3: Commit**

```bash
git add crates/rupu-app/src/window/sidebar.rs
git commit -m "feat(rupu-app): minimal accordion sidebar — 5 sections, project+global discovery"
```

---

## Task 14: Menubar status item stub

**Files:**
- Create: `crates/rupu-app/src/menu/mod.rs`
- Create: `crates/rupu-app/src/menu/menubar.rs`
- Modify: `crates/rupu-app/src/main.rs` (add `mod menu;` and call init)

GPUI doesn't (at the pinned commit) ship a menubar API — we drop down to `objc2` to install an `NSStatusItem`. For D-1 the menu just renders the rupu glyph + count "0"; the actual cross-workspace count + dropdown lands in D-3 / D-4.

> **macOS-only:** the menubar module is gated behind `cfg(target_os = "macos")`. On other platforms (Linux/Windows) for development builds, the init function is a no-op.

- [ ] **Step 1: Create `menu/mod.rs`**

```rust
// crates/rupu-app/src/menu/mod.rs
//! Application-level menus and menubar items.

pub mod menubar;
```

- [ ] **Step 2: Create `menu/menubar.rs`**

```rust
// crates/rupu-app/src/menu/menubar.rs
//! macOS menubar status item — the "cross-workspace runs badge".
//!
//! Spec §6.1 / §8.7: an always-on menubar item whose icon shows the
//! total in-flight run count across all open workspaces. For D-1
//! this is hard-wired to 0; D-3 lights up the count via a callback
//! the executor registers, and D-4 fills in the dropdown.

#[cfg(target_os = "macos")]
mod imp {
    use objc2::{rc::Retained, runtime::AnyObject, sel};
    use objc2_app_kit::{NSStatusBar, NSStatusItem, NSVariableStatusItemLength};
    use objc2_foundation::NSString;

    /// Install a status item in the system menubar. Returns the
    /// retained NSStatusItem so the caller (the App) keeps it alive
    /// for the process lifetime. Dropping the item removes it from
    /// the menubar.
    pub fn install() -> Retained<NSStatusItem> {
        // SAFETY: NSStatusBar::system + statusItemWithLength: are
        // Apple-documented entry points. The returned NSStatusItem
        // is retained by the status bar AND by us until we drop it.
        unsafe {
            let bar = NSStatusBar::systemStatusBar();
            let item = bar.statusItemWithLength(NSVariableStatusItemLength);

            // Title — for D-1 just the rupu glyph + the count.
            let title = NSString::from_str("◐ 0");
            if let Some(button) = item.button() {
                button.setTitle(&title);
            }

            item
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    /// No-op on non-macOS — the menubar is a Mac-only surface.
    /// Returns a unit type that the caller can store but never inspect.
    pub fn install() {}
}

pub use imp::install;
```

- [ ] **Step 3: Wire into `main.rs`**

```rust
mod menu;
mod palette;
mod window;
mod workspace;

use gpui::App;

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "rupu_app=debug,gpui=info".into()),
        )
        .init();

    let project_dir = std::env::args().nth(1)
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().expect("cwd"));

    let workspace = workspace::Workspace::open(&project_dir)
        .expect("open workspace");
    tracing::info!(id = %workspace.manifest.id, "opened workspace");

    App::new().run(move |cx| {
        cx.activate(true);

        // Install the menubar status item. Keep the retain alive for
        // the lifetime of the app loop — dropping it removes the
        // status item from the system menubar.
        #[cfg(target_os = "macos")]
        let _status_item = menu::menubar::install();

        window::WorkspaceWindow::open(workspace, cx);
    });
}
```

- [ ] **Step 4: Build + smoke run**

Run: `cargo build -p rupu-app && timeout 5 cargo run -p rupu-app -- /Users/matt/Code/Oracle/rupu || true`
Expected: window opens; system menubar (top of screen) shows `◐ 0` in the right-side status area. Disappears when the app exits.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-app/src/main.rs crates/rupu-app/src/menu
git commit -m "feat(rupu-app): menubar status item stub (◐ 0) via objc2 / NSStatusItem"
```

---

## Task 15: File menu — New / Open

**Files:**
- Create: `crates/rupu-app/src/menu/app_menu.rs`
- Modify: `crates/rupu-app/src/menu/mod.rs`
- Modify: `crates/rupu-app/src/main.rs`

D-1 ships the bare-minimum macOS app menu: `File > Open Workspace…` (folder picker → opens a window for that dir) and `File > New Workspace…` (folder picker that creates the dir if missing). `Open Recent` is intentionally deferred to D-2 — the data layer (`recents::list`) already exists, but wiring submenu items dynamically is enough complexity to bundle with the tab work that needs it.

> **GPUI menu APIs vary across pinned commits.** The pattern below uses `gpui::Menu`. If your version uses a different module path (`gpui::app_menu` or similar), adjust. The decisive shape: register a menu on app boot via `cx.set_menus(...)`; each item carries a `name` and an `action` token that the global action handler maps to a function.

- [ ] **Step 1: Create `menu/app_menu.rs`**

```rust
// crates/rupu-app/src/menu/app_menu.rs
//! macOS app menu — at the moment, just `File > New / Open`.
//! `Open Recent` lands in D-2 alongside the tab system.

use crate::window::WorkspaceWindow;
use crate::workspace::Workspace;
use gpui::{actions, App, Menu, MenuItem};

actions!(rupu_app, [NewWorkspace, OpenWorkspace, Quit]);

/// Register the menu and wire its action handlers. Call once on app boot.
pub fn install(cx: &mut App) {
    cx.set_menus(vec![Menu {
        name: "rupu".into(),
        items: vec![
            Menu {
                name: "File".into(),
                items: vec![
                    MenuItem::action("New Workspace…", NewWorkspace).into(),
                    MenuItem::action("Open Workspace…", OpenWorkspace).into(),
                    MenuItem::separator().into(),
                    MenuItem::action("Quit rupu", Quit).into(),
                ],
            }
            .into(),
        ],
    }]);

    cx.on_action(|_: &NewWorkspace, cx| {
        if let Some(dir) = pick_directory_modal("Choose a directory for the new workspace") {
            if let Err(e) = std::fs::create_dir_all(&dir) {
                tracing::error!(?dir, %e, "create workspace dir");
                return;
            }
            open_workspace_window(&dir, cx);
        }
    });
    cx.on_action(|_: &OpenWorkspace, cx| {
        if let Some(dir) = pick_directory_modal("Open a workspace directory") {
            open_workspace_window(&dir, cx);
        }
    });
    cx.on_action(|_: &Quit, cx| cx.quit());
}

fn open_workspace_window(dir: &std::path::Path, cx: &mut App) {
    match Workspace::open(dir) {
        Ok(workspace) => {
            tracing::info!(id = %workspace.manifest.id, path = ?dir, "open workspace");
            WorkspaceWindow::open(workspace, cx);
        }
        Err(e) => {
            tracing::error!(?dir, %e, "failed to open workspace");
            // TODO(D-2): surface this as a toast/modal once tab system exists.
        }
    }
}

/// Show a native NSOpenPanel directory picker. Returns Some(path) on
/// user confirm, None on cancel.
#[cfg(target_os = "macos")]
fn pick_directory_modal(prompt: &str) -> Option<std::path::PathBuf> {
    use objc2_app_kit::{NSModalResponseOK, NSOpenPanel};
    use objc2_foundation::NSString;

    unsafe {
        let panel = NSOpenPanel::openPanel();
        panel.setCanChooseDirectories(true);
        panel.setCanChooseFiles(false);
        panel.setAllowsMultipleSelection(false);
        panel.setCanCreateDirectories(true);
        let msg = NSString::from_str(prompt);
        panel.setMessage(Some(&msg));

        if panel.runModal() == NSModalResponseOK {
            let url = panel.URL()?;
            let path = url.path()?;
            Some(std::path::PathBuf::from(path.to_string()))
        } else {
            None
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn pick_directory_modal(_prompt: &str) -> Option<std::path::PathBuf> {
    // On non-macOS dev builds, fall back to env var so devs can
    // exercise the open flow without a native picker.
    std::env::var("RUPU_APP_OPEN_DIR").ok().map(std::path::PathBuf::from)
}
```

- [ ] **Step 2: Re-export from `menu/mod.rs`**

```rust
pub mod app_menu;
pub mod menubar;
```

- [ ] **Step 3: Wire into `main.rs`**

```rust
mod menu;
mod palette;
mod window;
mod workspace;

use gpui::App;

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "rupu_app=debug,gpui=info".into()),
        )
        .init();

    App::new().run(|cx| {
        cx.activate(true);

        menu::app_menu::install(cx);
        #[cfg(target_os = "macos")]
        let _status_item = menu::menubar::install();

        // If a directory was passed on the command line, open it
        // immediately. Otherwise wait for the user to pick via File menu.
        if let Some(arg) = std::env::args().nth(1) {
            let dir = std::path::PathBuf::from(arg);
            match workspace::Workspace::open(&dir) {
                Ok(workspace) => {
                    tracing::info!(id = %workspace.manifest.id, "opened workspace from CLI arg");
                    window::WorkspaceWindow::open(workspace, cx);
                }
                Err(e) => {
                    tracing::error!(?dir, %e, "failed to open workspace from CLI arg");
                }
            }
        }
    });
}
```

- [ ] **Step 4: Build + smoke run**

Run: `cargo build -p rupu-app && timeout 8 cargo run -p rupu-app || true`
Expected: app launches with no window. Menu bar shows `rupu > File > New Workspace… / Open Workspace… / Quit`. Pick `Open Workspace…`, choose a directory containing a `.rupu/` tree → window opens with that workspace's assets in the sidebar.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-app/src/main.rs crates/rupu-app/src/menu
git commit -m "feat(rupu-app): File menu — New / Open / Quit via NSOpenPanel"
```

---

## Task 16: Smoke test in CI

**Files:**
- Create: `crates/rupu-app/tests/fixtures/sample-workspace/.rupu/workflows/example.yaml`
- Create: `crates/rupu-app/tests/fixtures/sample-workspace/.rupu/agents/example.md`
- Modify: `Makefile`

The headless GPUI smoke pattern from Slice C: build the binary, spawn it with a fixture dir + a short timeout, assert clean exit. CI catches "the app no longer launches" regressions even though it can't verify pixels.

- [ ] **Step 1: Create the fixture workspace**

```yaml
# crates/rupu-app/tests/fixtures/sample-workspace/.rupu/workflows/example.yaml
name: example
description: D-1 smoke fixture — content is not parsed by the app shell.
steps:
  - id: hello
    agent: example
    prompt: hi
```

```markdown
<!-- crates/rupu-app/tests/fixtures/sample-workspace/.rupu/agents/example.md -->
---
name: example
provider: anthropic
model: claude-sonnet-4-6
---
You are a smoke-test agent.
```

- [ ] **Step 2: Add a Makefile target**

Append to `Makefile`:

```makefile
# rupu.app — headless smoke test. Builds the binary, launches it
# against the bundled fixture workspace, waits 4 seconds for the
# window to render, then SIGTERMs. Asserts no panic on stderr.
app-smoke:
	@cargo build --release -p rupu-app
	@FIXTURE="$$(pwd)/crates/rupu-app/tests/fixtures/sample-workspace"; \
	OUTPUT=$$(timeout 4 ./target/release/rupu-app "$$FIXTURE" 2>&1 || true); \
	if echo "$$OUTPUT" | grep -qE 'panic|panicked'; then \
		echo "app-smoke FAIL — panic in output:"; \
		echo "$$OUTPUT"; \
		exit 1; \
	fi
	@echo "app-smoke OK"
```

- [ ] **Step 3: Run the smoke locally**

Run: `make app-smoke`
Expected: prints `app-smoke OK`. (You'll briefly see the rupu.app window appear and disappear if running on macOS.)

- [ ] **Step 4: Commit**

```bash
git add crates/rupu-app/tests/fixtures Makefile
git commit -m "test(rupu-app): app-smoke make target + fixture workspace"
```

---

## Task 17: Docs — CLAUDE.md + README

**Files:**
- Modify: `CLAUDE.md`
- Modify: `README.md` (if rupu-app deserves a mention; otherwise just CLAUDE.md)

- [ ] **Step 1: Update CLAUDE.md crates list**

Find the `### Crates` section in `CLAUDE.md`. Add an entry for `rupu-app` alphabetically:

```markdown
- **`rupu-app`** — native macOS desktop app (Slice D). GPUI binary that opens workspaces, persists per-user state under `~/Library/Application Support/rupu.app/`, and renders the minimal-accordion sidebar / titlebar / menubar status item. D-1 is the workspace shell only — no tabs, no canvas, no orchestrator wiring (those come in D-2 / D-3 / D-6). Spec: `docs/superpowers/specs/2026-05-11-rupu-slice-d-app-design.md`.
```

Update the **Read first** section by appending the Slice D spec + Plan 1 pointers:

```markdown
- Slice D spec (native macOS app): `docs/superpowers/specs/2026-05-11-rupu-slice-d-app-design.md`
- Slice D Plan 1 (workspace shell, complete): `docs/superpowers/plans/2026-05-11-rupu-slice-d-plan-1-workspace-shell.md`
```

(Mark as "complete" only after Task 18 passes.)

- [ ] **Step 2: Commit**

```bash
git add CLAUDE.md
git commit -m "docs: CLAUDE.md — note rupu-app crate (Slice D Plan 1)"
```

---

## Task 18: Workspace gates

**Files:**
- (none — runs existing tooling)

- [ ] **Step 1: fmt**

Run: `cargo fmt --all -- --check`
Expected: no diff. If there are diffs, run `cargo fmt --all` and commit the result.

- [ ] **Step 2: clippy**

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: no warnings. The repo-wide `#![deny(clippy::all)]` lint enforces this; treat any warning as a build failure.

- [ ] **Step 3: tests**

Run: `cargo test --workspace`
Expected: all tests pass, including the new rupu-app tests (`palette` × 2, `manifest` × 2, `storage` × 2, `discovery` × 3, `recents` × 2, `workspace::handle` × 2 = 13 unit, plus 1 integration round-trip = 14 new tests).

- [ ] **Step 4: app-smoke**

Run: `make app-smoke`
Expected: `app-smoke OK`.

- [ ] **Step 5: final commit + flip the spec pointer**

If anything had to change during gates, commit it. Update `CLAUDE.md` Plan 1 pointer from "complete" if you marked it preemptively — otherwise mark it complete now.

```bash
git add CLAUDE.md
git commit -m "docs: mark Slice D Plan 1 complete in CLAUDE.md"
```

Push the branch + open the PR for review.

---

## Self-review notes

**Spec coverage check** (against `docs/superpowers/specs/2026-05-11-rupu-slice-d-app-design.md` §10 D-1 line):

| D-1 deliverable | Covered by |
|---|---|
| One window per workspace | Tasks 11 + 15 (window per `Workspace::open`; multiple opens → multiple windows) |
| Sidebar accordion | Task 13 |
| Menubar badge stub | Task 14 |
| Workspace manifest + persistence | Tasks 4, 5, 6 |

**Spec sections deferred to later sub-slices** (intentional; called out in the "Out of scope for this plan" block at the top):
- Tab content, view picker, drill-down → D-2..D-8
- Pane splits → D-2 alongside first tab
- `WorkflowExecutor` / `EventSink` traits → D-3
- `Open Recent` submenu → D-2 (data layer present in D-1 via `recents::list`)
- Workspace creation wizard with repo attachment + template picker → D-2
- Repos / Issues panels (connector-backed lists) → D-9
- Animations (`dot-pulse`, etc.) → D-3 (need live data first)
- Cross-workspace menubar dropdown → D-3 (need executor first)

**Placeholder scan:** there's one `TODO(D-2):` marker inside `open_workspace_window` in Task 15 — that's a deliberate forward-reference (no toast/modal layer exists in D-1; error is logged via tracing). Acceptable per the no-half-finished-implementations rule because the error path is genuinely partial pending D-2.

**Type consistency:** `Workspace` (handle, Task 9) → consumed by `WorkspaceWindow::open` (Task 11) → consumed by `app_menu::open_workspace_window` (Task 15). `WorkspaceColor::to_rgba()` defined in Task 4, used by titlebar in Task 12. `Asset` / `AssetSet` defined in Task 7, consumed by sidebar in Task 13.

**No-placeholder verification:** every code step has full executable Rust; every command step has the exact command + expected output; no "TBD" / "fill in later" / "add error handling" hand-waves.

---

Plan complete and saved to `docs/superpowers/plans/2026-05-11-rupu-slice-d-plan-1-workspace-shell.md`. Two execution options:

**1. Subagent-Driven (recommended)** — I dispatch a fresh subagent per task, review between tasks, fast iteration.

**2. Inline Execution** — Execute tasks in this session using executing-plans, batch execution with checkpoints.

Which approach?
