# Linux Packaging Phase A: `.deb` / `.rpm` as Release Assets — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Every release publishes four installable native packages — `.deb` and `.rpm`, each for amd64 and arm64 — that declare their dependencies and install completions and a man page.

**Architecture:** A `packages` job joins the existing release workflow, consuming the `asset-linux-*` artifacts it already produces. `nfpm` turns one YAML config into all four packages. The binary gains an `INSTALL_METHOD` build marker so packaged installs refuse `rupu update` and point at the system package manager instead.

**Tech Stack:** `nfpm`, GitHub Actions, `clap_mangen`, Rust 1.95 (pinned).

**Spec:** `docs/superpowers/specs/2026-07-29-rupu-linux-packaging-design.md`

**Depends on:** the release workflow from `.github/workflows/release.yml` (PR #560, merged) — specifically the `asset-linux-x64` / `asset-linux-arm64` artifacts and the `meta` job's `version` output.

**Scope:** Phase A only. Phase B (hosted APT/YUM repos, GPG signing) and Phase C (AUR, Homebrew, Nix) are separate plans; Phase B additionally blocks on a GPG key the maintainer must hold.

## Global Constraints

- **Nothing about the binary build changes.** Packages repackage the exact artifact the release workflow already built and tested in the pinned musl container. Never rebuild the binary in the packaging job.
- **Dependency classification is fixed by the spec** and derives from real failure behavior: `ripgrep` = **Depends** (hard error without it), `git` = **Recommends** (degrades gracefully; libgit2 is vendored), `ast-grep` = **Suggests** (returns empty output with a message).
- **Package name is `rupu`** and must never change — renaming breaks upgrades for everyone already installed.
- **Architecture strings differ by format.** `.deb` uses `amd64`/`arm64`; `.rpm` uses `x86_64`/`aarch64`. Assert the emitted filenames rather than assuming nfpm's translation.
- **Completions and the man page are architecture-independent.** Generate them once, from the x64 binary, and reuse for both packages — `ubuntu-latest` cannot execute the arm64 binary.
- `rupu-app` is macOS-only; scope any cargo command `-p rupu-cli` or `--workspace --exclude rupu-app`.
- Never run `cargo fmt` package-wide. Format only files you touched: `rustfmt --edition 2021 <file>`.
- Every change goes through a feature branch and a PR.

---

### Task 1: `INSTALL_METHOD` build marker

**Files:**
- Modify: `crates/rupu-cli/src/build_info.rs`

**Interfaces:**
- Produces: `build_info::INSTALL_METHOD: Option<&str>`, `build_info::package_manager() -> Option<&'static str>`, and the pure `build_info::package_manager_for(Option<&str>) -> Option<&'static str>`.

- [ ] **Step 1: Write the failing tests**

Add to the existing `mod tests` in `crates/rupu-cli/src/build_info.rs`:

```rust
    #[test]
    fn package_manager_maps_each_packaging_format() {
        assert_eq!(package_manager_for(Some("deb")), Some("apt"));
        assert_eq!(package_manager_for(Some("rpm")), Some("dnf"));
    }

    #[test]
    fn package_manager_is_none_for_unpackaged_installs() {
        // Tarball, install.sh, `cargo install`, and dev builds all leave the
        // marker unset — none of them are owned by a package manager, so
        // `rupu update` must keep working for them.
        assert_eq!(package_manager_for(None), None);
        assert_eq!(package_manager_for(Some("")), None);
        assert_eq!(package_manager_for(Some("tarball")), None);
    }

    #[test]
    fn no_install_method_under_cargo_test() {
        assert_eq!(INSTALL_METHOD, None);
        assert_eq!(package_manager(), None);
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p rupu-cli --lib build_info`
Expected: FAIL — `cannot find function 'package_manager_for' in this scope`

- [ ] **Step 3: Write the implementation**

Add to `crates/rupu-cli/src/build_info.rs`, after `RELEASE_VERSION`:

```rust
/// "deb" | "rpm" when this binary was installed from a native package;
/// `None` for tarball, `install.sh`, `cargo install`, and dev builds.
///
/// Stamped by the packaging job exactly the way `RUPU_RELEASE_CHANNEL` is
/// stamped by the release build — same mechanism, no new pattern.
pub const INSTALL_METHOD: Option<&str> = option_env!("RUPU_INSTALL_METHOD");

/// The system package manager that owns this binary, if any.
///
/// Kept pure and separate from [`package_manager`] so the whole mapping is
/// testable — `option_env!` is fixed at compile time and cannot be varied
/// from a test.
pub fn package_manager_for(method: Option<&str>) -> Option<&'static str> {
    match method {
        Some("deb") => Some("apt"),
        Some("rpm") => Some("dnf"),
        _ => None,
    }
}

/// The system package manager that owns this binary, if any.
pub fn package_manager() -> Option<&'static str> {
    package_manager_for(INSTALL_METHOD)
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p rupu-cli --lib build_info`
Expected: PASS — four tests including the pre-existing `dev_build_when_env_absent`.

- [ ] **Step 5: Format and commit**

```bash
rustfmt --edition 2021 crates/rupu-cli/src/build_info.rs
git add crates/rupu-cli/src/build_info.rs
git commit -m "feat(cli): INSTALL_METHOD build marker for packaged installs"
```

---

### Task 2: `rupu update` refuses on packaged installs

**Files:**
- Modify: `crates/rupu-cli/src/cmd/update.rs` (`UpdateArgs` is at ~line 23; `async fn run` at ~line 77)

**Interfaces:**
- Consumes: `build_info::package_manager()` from Task 1.
- Produces: `update::packaged_refusal(pm: Option<&str>, check: bool, rollback: bool) -> Option<String>` — `Some(message)` when the command must be refused.

- [ ] **Step 1: Write the failing tests**

Add a `mod tests` at the bottom of `crates/rupu-cli/src/cmd/update.rs` (or extend it if one exists):

```rust
#[cfg(test)]
mod packaged_tests {
    use super::*;

    #[test]
    fn unpackaged_installs_are_never_refused() {
        // Tarball / install.sh / dev builds own their own binary.
        assert!(packaged_refusal(None, false, false).is_none());
        assert!(packaged_refusal(None, true, false).is_none());
        assert!(packaged_refusal(None, false, true).is_none());
    }

    #[test]
    fn packaged_install_refuses_an_actual_update() {
        let msg = packaged_refusal(Some("apt"), false, false).expect("must refuse");
        assert!(msg.contains("apt upgrade rupu"), "message was: {msg}");
        assert!(msg.contains("--check"), "must point at the still-working alternative");
    }

    #[test]
    fn packaged_install_refuses_rollback() {
        // Rolling back under a package manager leaves it disagreeing with
        // what is on disk, and the next upgrade silently undoes it.
        let msg = packaged_refusal(Some("dnf"), false, true).expect("must refuse");
        assert!(msg.contains("dnf"), "message was: {msg}");
    }

    #[test]
    fn packaged_install_still_allows_check() {
        // A user must be able to learn they are behind regardless of how
        // they installed.
        assert!(packaged_refusal(Some("apt"), true, false).is_none());
    }

    #[test]
    fn the_message_names_the_right_package_manager() {
        let apt = packaged_refusal(Some("apt"), false, false).unwrap();
        let dnf = packaged_refusal(Some("dnf"), false, false).unwrap();
        assert!(apt.contains("apt") && !apt.contains("dnf"));
        assert!(dnf.contains("dnf") && !dnf.contains("apt"));
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p rupu-cli --lib packaged_tests`
Expected: FAIL — `cannot find function 'packaged_refusal' in this scope`

- [ ] **Step 3: Write the implementation**

Add to `crates/rupu-cli/src/cmd/update.rs`, above `async fn run`:

```rust
/// Whether `rupu update` must refuse, and what to say.
///
/// `--check` is deliberately always allowed: a user should be able to learn
/// they are behind no matter how they installed. Everything that would
/// *write* the binary is refused, because the package manager owns
/// `/usr/bin/rupu` — a self-update needs root and would be silently
/// reverted by the next upgrade, leaving the version to go backwards for
/// no visible reason.
///
/// Pure so the whole matrix is testable; `INSTALL_METHOD` is fixed at
/// compile time and cannot be varied from a test.
fn packaged_refusal(pm: Option<&str>, check: bool, rollback: bool) -> Option<String> {
    let pm = pm?;
    if check && !rollback {
        return None;
    }
    let action = if rollback { "roll back" } else { "update" };
    Some(format!(
        "rupu was installed from a system package, so it cannot {action} itself — \
         the next `{pm} upgrade` would overwrite whatever it wrote.\n  \
         Run `sudo {pm} upgrade rupu` instead.\n  \
         `rupu update --check` still works and will tell you if a newer version exists."
    ))
}
```

Then wire it into `run`, immediately after the `print_platform` short-circuit and **before** `load_cli_config()`:

```rust
    if let Some(message) = packaged_refusal(
        crate::build_info::package_manager(),
        args.check,
        args.rollback,
    ) {
        anyhow::bail!(message);
    }
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p rupu-cli --lib packaged_tests`
Expected: PASS — five tests.

- [ ] **Step 5: Confirm the normal path is unaffected**

Run: `cargo run -p rupu-cli -- update --print-platform`
Expected: prints `darwin-arm64` (or the host's platform). The marker is unset in a dev build, so nothing is refused.

- [ ] **Step 6: Format and commit**

```bash
rustfmt --edition 2021 crates/rupu-cli/src/cmd/update.rs
git add crates/rupu-cli/src/cmd/update.rs
git commit -m "feat(update): refuse self-update on packaged installs"
```

---

### Task 3: `rupu man` — generate the man page

There is no man page today. `clap_mangen` renders one from the same clap `Command` that defines the CLI, so it cannot drift from the actual flags. A hidden subcommand mirrors `rupu completions print`, which the packaging job already has to call.

**Files:**
- Modify: root `Cargo.toml` (`[workspace.dependencies]`)
- Modify: `crates/rupu-cli/Cargo.toml`
- Create: `crates/rupu-cli/src/cmd/man.rs`
- Modify: `crates/rupu-cli/src/lib.rs` (register the module and the subcommand)

**Interfaces:**
- Produces: `rupu man` writing roff to stdout, exit 0. The packaging job pipes it to `rupu.1`.

- [ ] **Step 1: Write the failing test**

Create `crates/rupu-cli/tests/cli_man.rs`:

```rust
//! `rupu man` renders the man page the packaging job installs. It is
//! generated from the live clap Command, so it cannot drift from the CLI.

use assert_cmd::Command;

#[test]
fn man_emits_a_roff_document() {
    let out = Command::cargo_bin("rupu")
        .expect("rupu binary builds")
        .arg("man")
        .output()
        .expect("run rupu man");

    assert!(out.status.success(), "rupu man must exit 0");
    let roff = String::from_utf8(out.stdout).expect("roff is utf-8");

    // `.TH` is the man-page title header — the first thing troff needs.
    assert!(roff.starts_with(".TH"), "expected a .TH header, got: {:?}", &roff[..roff.len().min(80)]);
    assert!(roff.contains("rupu"), "must document the rupu command");
    assert!(!roff.is_empty());
}

#[test]
fn man_documents_a_known_subcommand() {
    // Guards against rendering an empty shell of a page: `update` is a real
    // subcommand and must appear.
    let out = Command::cargo_bin("rupu")
        .expect("rupu binary builds")
        .arg("man")
        .output()
        .expect("run rupu man");
    let roff = String::from_utf8(out.stdout).unwrap();
    assert!(roff.contains("update"), "man page should mention the update subcommand");
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p rupu-cli --test cli_man`
Expected: FAIL — clap rejects `man` as an unknown subcommand, so the process exits non-zero.

- [ ] **Step 3: Add the dependency**

In root `Cargo.toml`, next to the existing `clap` / `clap_complete` entries in `[workspace.dependencies]`:

```toml
clap_mangen = "0.2"
```

In `crates/rupu-cli/Cargo.toml`, next to `clap_complete.workspace = true`:

```toml
clap_mangen.workspace = true
```

- [ ] **Step 4: Write the module**

Create `crates/rupu-cli/src/cmd/man.rs`:

```rust
//! `rupu man` — render the man page to stdout.
//!
//! Generated from the live clap `Command`, so the page cannot describe
//! flags the binary does not have. The packaging job pipes this to
//! `rupu.1` and installs it; there is no checked-in man source to drift.

use clap::CommandFactory;
use std::io::Write;
use std::process::ExitCode;

pub fn handle() -> ExitCode {
    let mut buf: Vec<u8> = Vec::new();
    if let Err(e) = clap_mangen::Man::new(crate::Cli::command()).render(&mut buf) {
        eprintln!("rupu man: render failed: {e}");
        return ExitCode::from(1);
    }
    if let Err(e) = std::io::stdout().write_all(&buf) {
        eprintln!("rupu man: write failed: {e}");
        return ExitCode::from(1);
    }
    ExitCode::SUCCESS
}
```

- [ ] **Step 5: Register the module and subcommand**

In `crates/rupu-cli/src/cmd/mod.rs`, add alongside the other module declarations:

```rust
pub mod man;
```

In `crates/rupu-cli/src/lib.rs`, add a variant to the `Cmd` enum. Mark it hidden — it exists for packaging, not for users:

```rust
    /// Render the man page to stdout.
    #[command(hide = true)]
    Man,
```

and a dispatch arm wherever the other `Cmd` variants are matched:

```rust
        Cmd::Man => cmd::man::handle(),
```

Match the surrounding arms' shape — if they are `async` or return `ExitCode` via a different wrapper, follow that rather than this literal line.

- [ ] **Step 6: Run the test to verify it passes**

Run: `cargo test -p rupu-cli --test cli_man`
Expected: PASS — both tests.

- [ ] **Step 7: Eyeball the rendered page**

```bash
cargo run -p rupu-cli -- man | head -20
cargo run -p rupu-cli -- man | man /dev/stdin | head -20
```
Expected: a `.TH rupu 1 ...` header, then a formatted page. The second command renders it the way a user would read it.

- [ ] **Step 8: Format and commit**

```bash
rustfmt --edition 2021 crates/rupu-cli/src/cmd/man.rs crates/rupu-cli/src/lib.rs
git add -A
git commit -m "feat(cli): rupu man — render the man page from the clap Command"
```

---

### Task 4: nfpm config and the package build script

**Files:**
- Create: `packaging/nfpm.yaml`
- Create: `packaging/build-packages.sh`

**Interfaces:**
- Consumes: a staged directory `dist/` containing `rupu` (the binary), `completions/`, and `rupu.1`.
- Produces: `packaging/build-packages.sh <version> <arch>` emitting `dist/rupu_<version>_<debarch>.deb` and `dist/rupu-<version>.<rpmarch>.rpm`.

- [ ] **Step 1: Write the nfpm config**

Create `packaging/nfpm.yaml`:

```yaml
# nfpm turns one config into both .deb and .rpm from an ALREADY-BUILT
# binary. The binary is never rebuilt here — it is the exact artifact the
# release workflow produced and tested in the pinned musl container.
name: rupu
arch: ${PKG_ARCH}
platform: linux
version: ${PKG_VERSION}
section: devel
priority: optional
maintainer: Section9Labs <matias@section9labs.com>
description: |
  Agentic code-development CLI.
  Runs agents and workflows against your repositories from the terminal.
vendor: Section9Labs
homepage: https://github.com/Section9Labs/rupu
license: Apache-2.0

# Classified from real failure behavior, not assumption:
#   ripgrep  - crates/rupu-tools/src/grep.rs hard-errors without `rg`
#   git      - cmd/init.rs degrades gracefully; libgit2 is vendored, so core
#              repository operations never shell out to the git binary
#   ast-grep - returns empty output with an explanatory message when absent
depends:
  - ripgrep
recommends:
  - git
suggests:
  - ast-grep

contents:
  - src: ./dist/rupu
    dst: /usr/bin/rupu
    file_info:
      mode: 0755

  - src: ./dist/completions/rupu.bash
    dst: /usr/share/bash-completion/completions/rupu
  - src: ./dist/completions/_rupu
    dst: /usr/share/zsh/site-functions/_rupu
  - src: ./dist/completions/rupu.fish
    dst: /usr/share/fish/vendor_completions.d/rupu.fish

  - src: ./dist/rupu.1.gz
    dst: /usr/share/man/man1/rupu.1.gz

  - src: ./LICENSE
    dst: /usr/share/doc/rupu/LICENSE
  - src: ./README.md
    dst: /usr/share/doc/rupu/README.md
```

- [ ] **Step 2: Write the build script**

Create `packaging/build-packages.sh`:

```bash
#!/usr/bin/env bash
# packaging/build-packages.sh <version> <nfpm-arch>
#
# Stages an already-built rupu binary plus its generated completions and man
# page, then emits a .deb and an .rpm. The binary is NEVER rebuilt here.
#
# Completions and the man page are architecture-independent — they are
# generated once from the x64 binary by the caller and reused for both
# architectures, because an x86_64 runner cannot execute the arm64 binary.
set -euo pipefail

VERSION="${1:?usage: build-packages.sh <version> <amd64|arm64>}"
PKG_ARCH="${2:?usage: build-packages.sh <version> <amd64|arm64>}"

case "$PKG_ARCH" in
  amd64|arm64) ;;
  *) echo "arch must be amd64 or arm64 (got: $PKG_ARCH)" >&2; exit 1 ;;
esac

test -x dist/rupu                        || { echo "dist/rupu missing or not executable" >&2; exit 1; }
test -f dist/rupu.1.gz                   || { echo "dist/rupu.1.gz missing" >&2; exit 1; }
test -f dist/completions/rupu.bash       || { echo "dist/completions/rupu.bash missing" >&2; exit 1; }
test -f dist/completions/_rupu           || { echo "dist/completions/_rupu missing" >&2; exit 1; }
test -f dist/completions/rupu.fish       || { echo "dist/completions/rupu.fish missing" >&2; exit 1; }

export PKG_VERSION="$VERSION"
export PKG_ARCH

echo "→ building packages for $PKG_ARCH at $PKG_VERSION"
nfpm package --config packaging/nfpm.yaml --packager deb --target dist/
nfpm package --config packaging/nfpm.yaml --packager rpm --target dist/

echo "→ produced:"
ls -la dist/*.deb dist/*.rpm
```

- [ ] **Step 3: Make it executable and verify the shell parses**

```bash
chmod +x packaging/build-packages.sh
bash -n packaging/build-packages.sh && echo "shell syntax ok"
```

- [ ] **Step 4: Verify the config is valid YAML**

Run: `python3 -c "import yaml; yaml.safe_load(open('packaging/nfpm.yaml')); print('yaml ok')"`
Expected: `yaml ok`

- [ ] **Step 5: Commit**

```bash
git add packaging/
git commit -m "build(packaging): nfpm config and package build script"
```

---

### Task 5: The `packages` job in the release workflow

**Files:**
- Modify: `.github/workflows/release.yml`

**Interfaces:**
- Consumes: `meta.outputs.version`; artifacts `asset-linux-x64` and `asset-linux-arm64` from the `linux` job.
- Produces: artifact `asset-packages` containing four package files, picked up by the existing `publish` job.

- [ ] **Step 1: Add the job**

Insert after the `linux` job in `.github/workflows/release.yml`:

```yaml
  packages:
    name: build .deb and .rpm
    needs: [meta, linux]
    runs-on: ubuntu-latest
    timeout-minutes: 30
    steps:
      - uses: actions/checkout@v5

      - name: Install nfpm
        run: |
          set -euo pipefail
          curl -sSfL -o /tmp/nfpm.deb \
            https://github.com/goreleaser/nfpm/releases/download/v2.43.0/nfpm_2.43.0_amd64.deb
          sudo dpkg -i /tmp/nfpm.deb
          nfpm --version

      - uses: actions/download-artifact@v4
        with:
          name: asset-linux-x64
          path: staging/x64
      - uses: actions/download-artifact@v4
        with:
          name: asset-linux-arm64
          path: staging/arm64

      # Completions and the man page are architecture-independent, and this
      # runner cannot execute the arm64 binary — so generate once from x64
      # and reuse for both packages.
      - name: Generate completions and man page (once, from the x64 binary)
        run: |
          set -euo pipefail
          chmod +x staging/x64/rupu-linux-x64
          bin=./staging/x64/rupu-linux-x64
          mkdir -p shared/completions
          "$bin" completions print bash --static > shared/completions/rupu.bash
          "$bin" completions print zsh  --static > shared/completions/_rupu
          "$bin" completions print fish --static > shared/completions/rupu.fish
          "$bin" man > shared/rupu.1
          gzip -9 -n -f shared/rupu.1
          ls -la shared shared/completions

      - name: Build all four packages
        env:
          VERSION: ${{ needs.meta.outputs.version }}
        run: |
          set -euo pipefail
          # nfpm rejects a leading `v` and pre-release shapes vary; strip any
          # leading v and keep the rest.
          ver="${VERSION#v}"
          mkdir -p out
          for pair in "x64:amd64:rupu-linux-x64" "arm64:arm64:rupu-linux-arm64"; do
            src="${pair%%:*}"; rest="${pair#*:}"
            arch="${rest%%:*}"; binname="${rest#*:}"
            rm -rf dist && mkdir -p dist/completions
            cp "staging/$src/$binname" dist/rupu
            chmod +x dist/rupu
            cp shared/completions/* dist/completions/
            cp shared/rupu.1.gz dist/rupu.1.gz
            ./packaging/build-packages.sh "$ver" "$arch"
            mv dist/*.deb dist/*.rpm out/
          done
          ls -la out

      # The architecture strings differ by format — .deb uses amd64/arm64,
      # .rpm uses x86_64/aarch64. A mismatch ships a package that installs on
      # the wrong machine, so assert rather than assume.
      - name: Assert the emitted filenames
        run: |
          set -euo pipefail
          ls out
          test -n "$(ls out/*amd64.deb 2>/dev/null)"   || { echo "::error::no amd64 .deb"; exit 1; }
          test -n "$(ls out/*arm64.deb 2>/dev/null)"   || { echo "::error::no arm64 .deb"; exit 1; }
          test -n "$(ls out/*x86_64.rpm 2>/dev/null)"  || { echo "::error::no x86_64 .rpm"; exit 1; }
          test -n "$(ls out/*aarch64.rpm 2>/dev/null)" || { echo "::error::no aarch64 .rpm"; exit 1; }
          test "$(ls out | wc -l)" -eq 4 || { echo "::error::expected exactly 4 packages"; exit 1; }

      - uses: actions/upload-artifact@v4
        with:
          name: asset-packages
          path: out/
          if-no-files-found: error
```

- [ ] **Step 2: Stamp the install method into the packaged binary**

The binary in the package must carry `RUPU_INSTALL_METHOD`. It is built by the `linux` job, which does not know about packaging — so the `linux` job needs a second build for packaging use.

In the `linux` job, after the existing `Build (channel-stamped)` step, add:

```yaml
      - name: Build again with the package install marker
        run: |
          docker run --rm \
            -e RUPU_RELEASE_CHANNEL="${{ needs.meta.outputs.channel }}" \
            -e RUPU_RELEASE_VERSION="${{ needs.meta.outputs.version }}" \
            -e RUPU_INSTALL_METHOD=deb \
            -v "$PWD":/work -w /work rupu-linux-build \
            cargo build --release -p rupu-cli --target-dir target/linux-musl-pkg
          mkdir -p dist
          cp target/linux-musl-pkg/release/rupu "dist/rupu-${{ matrix.platform }}-pkg"
```

and add `dist/rupu-${{ matrix.platform }}-pkg` to the artifact upload path.

**Note the `deb` value is used for the `.rpm` too.** That is wrong — `package_manager()` would tell an rpm user to run `apt`. Resolve it by building a third time with `RUPU_INSTALL_METHOD=rpm`, or by having Task 1's mapping accept a single `pkg` value and detect apt-vs-dnf at runtime. Pick one and make it explicit; do not leave the rpm binary saying "apt".

- [ ] **Step 3: Add the packages to the publish job**

In the `publish` job the download already uses `pattern: asset-*`, so `asset-packages` is collected automatically. Extend the sanity check so a missing package fails the release rather than publishing a partial set:

```yaml
          test "$(ls dist/*.deb dist/*.rpm 2>/dev/null | wc -l)" -eq 4 \
            || { echo "::error::expected 4 packages"; exit 1; }
```

- [ ] **Step 4: Verify the YAML parses and every run block is valid shell**

```bash
python3 -c "import yaml; yaml.safe_load(open('.github/workflows/release.yml')); print('yaml ok')"
```

Then extract each `run:` block and check it with `bash -n` — the loop in Step 1 is easy to break with YAML dedenting.

- [ ] **Step 5: Commit**

```bash
git add .github/workflows/release.yml
git commit -m "ci(release): build .deb and .rpm for both architectures"
```

---

### Task 6: End-to-end verification in real distro containers

A package that builds is not a package that installs. This task runs the actual install on the actual distros.

**Files:**
- Modify: `.github/workflows/release.yml` (add verification steps to the `packages` job)

**Interfaces:**
- Consumes: the four packages from Task 5.

- [ ] **Step 1: Add a Debian install check**

Append to the `packages` job:

```yaml
      - name: Verify the .deb installs and runs on Debian
        run: |
          set -euo pipefail
          docker run --rm -v "$PWD/out":/pkg debian:12 bash -c '
            set -euo pipefail
            apt-get update -qq
            # `apt-get install ./file.deb` resolves declared dependencies,
            # which is the whole point of shipping a package rather than a
            # tarball — this proves ripgrep gets pulled in.
            apt-get install -y -qq /pkg/*amd64.deb
            command -v rg    >/dev/null || { echo "ripgrep dependency was not installed"; exit 1; }
            rupu --version
            test -f /usr/share/bash-completion/completions/rupu || { echo "bash completions missing"; exit 1; }
            test -f /usr/share/man/man1/rupu.1.gz              || { echo "man page missing"; exit 1; }
            # The packaged binary must refuse to update itself.
            if rupu update 2>&1 | grep -q "apt upgrade rupu"; then
              echo "packaged install correctly refuses self-update"
            else
              echo "::error::packaged install did not refuse rupu update"; exit 1
            fi
            rupu update --check || true   # --check must still be permitted
          '
```

- [ ] **Step 2: Add a Fedora install check**

```yaml
      - name: Verify the .rpm installs and runs on Fedora
        run: |
          set -euo pipefail
          docker run --rm -v "$PWD/out":/pkg fedora:41 bash -c '
            set -euo pipefail
            dnf install -y -q /pkg/*x86_64.rpm
            command -v rg >/dev/null || { echo "ripgrep dependency was not installed"; exit 1; }
            rupu --version
            test -f /usr/share/man/man1/rupu.1.gz || { echo "man page missing"; exit 1; }
            if rupu update 2>&1 | grep -qE "dnf upgrade rupu|apt upgrade rupu"; then
              echo "packaged install correctly refuses self-update"
            else
              echo "::error::packaged install did not refuse rupu update"; exit 1
            fi
          '
```

If the rpm binary reports `apt`, that is Task 5 Step 2's unresolved marker problem surfacing — fix it there rather than loosening this check permanently.

- [ ] **Step 3: Verify the YAML parses**

Run: `python3 -c "import yaml; yaml.safe_load(open('.github/workflows/release.yml')); print('yaml ok')"`

- [ ] **Step 4: Commit and exercise with a real release**

```bash
git add .github/workflows/release.yml
git commit -m "ci(release): verify packages install on Debian and Fedora"
```

Then cut a beta and confirm the release carries ten assets — three binaries, three `.sha256`, and four packages.

---

## Self-Review

**Spec coverage.**

| Spec section | Task |
|---|---|
| §1 binary at `/usr/bin/rupu` | 4 (nfpm `contents`) |
| §1 bash/zsh/fish completions at system paths | 4, 5 |
| §1 man page | 3, 4, 5 |
| §1 `/usr/share/doc/rupu` | 4 |
| §1 `ripgrep` Depends / `git` Recommends / `ast-grep` Suggests | 4 |
| §2 nfpm, one config, no rebuild of the binary | 4, 5 |
| §2 four packages, arch naming asserted | 5 Step 4 |
| §4 `RUPU_INSTALL_METHOD` marker | 1, 5 Step 2 |
| §4 `rupu update` refuses; `--check` still works; `--rollback` refuses | 2 |
| §6 Phase A scope | whole plan |

Spec §3 (signing, hosted repos) is Phase B and §5 (AUR/Homebrew/Nix) is Phase C — deliberately absent, not gaps.

**Placeholder scan.** No "TBD"/"TODO". One item is deliberately left as an explicit decision rather than a silent default: Task 5 Step 2 flags that stamping `RUPU_INSTALL_METHOD=deb` into the binary used for *both* packages would make an rpm install advise `apt`. Two concrete resolutions are given; the implementer must choose one and say which. Leaving it unstated would have shipped the wrong advice to Fedora users.

**Type consistency.** `package_manager_for(Option<&str>) -> Option<&'static str>` is defined in Task 1 and consumed by name in Task 2's wiring. `packaged_refusal(pm, check, rollback) -> Option<String>` is defined and consumed within Task 2. The staged layout `dist/{rupu, rupu.1.gz, completions/{rupu.bash,_rupu,rupu.fish}}` is written identically in Task 4's script guards, Task 4's nfpm `contents`, and Task 5's staging loop. Artifact name `asset-packages` matches between Task 5's upload and the `publish` job's existing `asset-*` pattern.

**Known risk carried from the spec.** `ripgrep` is in Fedora proper but on RHEL/CentOS derivatives it lives in EPEL, so `dnf install` of the rpm may fail to resolve there. Task 6 verifies Fedora only; RHEL support would need either an EPEL note in the docs or a vendored `rg`. Out of scope for Phase A, but worth knowing before someone reports it.
