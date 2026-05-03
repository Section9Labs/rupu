# Releasing rupu (manual runbook)

rupu uses a build-locally-and-upload runbook for releases. There is no automated GitHub Actions release workflow — three attempts at the v0.0.3-cli release surfaced enough churn (MSRV-vs-transitive-dep drift twice, cross-stdlib quirks on macos-14) that the runbook is honester for a solo-maintained v0. When external-contributor flow needs automation, reinstate `.github/workflows/release.yml` (the deleted version is in git history at commit `6c30b31`).

## What you need

- A workstation with Rust ≥ MSRV (currently 1.88).
- `gh` authenticated (`gh auth status`).
- Push access to `Section9Labs/rupu`.

## Steps

### 1. Bump version (if applicable)

In `Cargo.toml` `[workspace.package]`, set `version = "X.Y.Z"`. Stage and commit on a branch; merge via PR (this isn't manual-runbook work, just normal change control).

### 2. Verify clean state

```bash
git checkout main && git pull
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
cargo build --release --workspace
```

All four must be green.

### 3. Tag

```bash
TAG=vX.Y.Z-<slug>     # e.g., v0.0.3-cli
git tag -a "$TAG" -m "<one-line release summary>"
git push origin "$TAG"
```

If SSH push fails (1Password agent timeout), use HTTPS:
```bash
git -c url."https://github.com/".insteadOf="git@github.com:" push origin "$TAG"
```

### 4. Build the binary natively (one per platform you ship)

For your current platform:

```bash
make release                       # cargo build --release + sign with Developer ID
cd target/release
strip rupu                         # smaller binary; no debug symbols
TARGET=$(rustc -vV | awk '/^host/ { print $2 }')   # e.g., aarch64-apple-darwin
NAME="rupu-${TAG}-${TARGET}"
tar -czf "${NAME}.tar.gz" rupu
shasum -a 256 "${NAME}.tar.gz" > "${NAME}.tar.gz.sha256"
ls -la "${NAME}.tar.gz"*
```

`make release` runs `cargo build --release -p rupu-cli` and then `scripts/sign-dev.sh release`, which signs the binary with the Developer ID Application cert and the hardened runtime. Required for both the keychain-trust workflow (so successive builds don't re-prompt) and notarization in step 4a.

#### 4a. Notarize the macOS binary (one-time prereq + per-release submit)

**One-time setup** (skip if you've already done this once):

```bash
xcrun notarytool store-credentials rupu \
  --apple-id <your@apple.id> \
  --team-id 995PCLM9KH
```

You'll be prompted for an [app-specific password](https://appleid.apple.com/account/manage) — generate one under "Sign-In and Security → App-Specific Passwords" and paste it. Stored in your login keychain as profile `rupu`.

**Per release** (run after step 4's sign step, before tarring):

```bash
scripts/notarize-release.sh
```

This wraps `target/release/rupu` in a temp .zip, submits to `xcrun notarytool` with the `rupu` keychain profile, waits for the verdict, and exits non-zero on failure (printing the notarization log). On success the binary's signature carries the notarization ticket online; Gatekeeper will accept it on first run for end users.

There's no `stapler` step — bare command-line binaries can't be stapled (stapler only attaches tickets to `.app`/`.pkg`/`.dmg`). The online notarization check via Gatekeeper covers this.

Repeat steps 4 + 4a on each host you have access to (macOS arm64 + Intel; Linux x86_64; Linux arm64). Linux/Windows skip the sign + notarize steps via the script's OS check. v0 only ships what you happen to build; users on unsupported platforms install via `cargo install --git https://github.com/Section9Labs/rupu --tag $TAG`.

### 5. Create the GitHub release

For the first artifact, use `gh release create`:

```bash
gh release create "$TAG" --repo Section9Labs/rupu \
  --title "rupu $TAG" \
  --notes "$(cat <<'EOF'
**Slice X complete.** <one-paragraph summary>

## What ships in this binary

- <feature 1>
- <feature 2>

## Install

\`\`\`bash
TAG=vX.Y.Z-slug
TARGET=aarch64-apple-darwin    # adjust for your platform
curl -fsSL -o /tmp/rupu.tar.gz \
  "https://github.com/Section9Labs/rupu/releases/download/\${TAG}/rupu-\${TAG}-\${TARGET}.tar.gz"
tar -xzf /tmp/rupu.tar.gz -C /tmp
sudo install -m 755 /tmp/rupu /usr/local/bin/rupu
rupu --version
\`\`\`
EOF
)" \
  "${NAME}.tar.gz" "${NAME}.tar.gz.sha256"
```

For additional artifacts on the same release, use `gh release upload`:

```bash
gh release upload "$TAG" --repo Section9Labs/rupu \
  "rupu-${TAG}-other-target.tar.gz" \
  "rupu-${TAG}-other-target.tar.gz.sha256"
```

### 6. Smoke the release

On a fresh checkout of a different machine (or just `/tmp`):

```bash
TAG=vX.Y.Z-slug
TARGET=aarch64-apple-darwin
curl -fsSL "https://github.com/Section9Labs/rupu/releases/download/${TAG}/rupu-${TAG}-${TARGET}.tar.gz" \
  | tar -xzf - -C /tmp
/tmp/rupu --version    # rupu X.Y.Z
```

## Why no GitHub Actions release workflow?

Three release attempts on `v0.0.3-cli` each hit a different transient or environmental issue:

1. Build failed on all 4 targets: `feature edition2024 is required` because `Cargo.lock` pinned `base64ct@1.8.3` etc., which require Rust 1.85+. Our MSRV pin was 1.77. Fixed by PR #9 (MSRV → 1.85).
2. Build failed on all 4 targets again: `home@0.5.12 requires rustc 1.88`. Fixed by PR #10 (MSRV → 1.88).
3. `x86_64-apple-darwin` failed cross-compiling from `macos-14` (arm64 host) — `targets: x86_64-apple-darwin` didn't actually install the cross-stdlib. Fixed by PR #11 (switched to `macos-13` Intel runner).

Each fix was small (a one-line MSRV bump or a runner swap), but the iteration cost was real: every retry meant deleting the existing tag, re-pushing it, and waiting for runners to pick up. The native build path takes ~10 seconds per binary on a workstation, with no surprises.

When external-contributor flow needs automation (Slice C+), reinstate the workflow from git history (`git show 6c30b31:.github/workflows/release.yml`).
