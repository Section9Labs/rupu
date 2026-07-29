# Cross-Platform Plan 3: CI Release Pipeline Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** One tag push publishes `rupu-darwin-arm64`, `rupu-linux-x64`, and `rupu-linux-arm64` together, with macOS signed and notarized, and `install.sh` to fetch the right one.

**Architecture:** A tag-triggered workflow derives the channel from the tag suffix, builds the CP web UI **once** and shares it as an artifact so all three binaries embed a byte-identical UI, builds three binaries on native runners, then publishes every asset from a single job so a partial release cannot happen. `rupu update` needs no changes: the existing rolling + versioned two-tag scheme is preserved exactly.

**Tech Stack:** GitHub Actions, the pinned Alpine/musl image from Plan 2, `codesign` + `notarytool`, `gh release`.

**Spec:** `docs/superpowers/specs/2026-07-27-rupu-cross-platform-release-design.md` §4, §6

**Depends on:** Plan 2 (#559, merged). The musl image and the `--print-platform` naming contract are prerequisites.

## Global Constraints

- **Asset names come from the binary, never from `uname` or a literal.** `rupu update --print-platform` is the single source (#555). A second derivation is the defect that motivated this arc.
- **`make cp-web` must run before any release build.** Otherwise `rupu-cp` embeds its "not built" placeholder and the published binary ships a dead UI. Verified in Plan 2's CI: the gate deliberately skips it and logs a warning saying so.
- **The two-tag scheme is a compatibility contract.** `rupu update` resolves releases by semver + the `prerelease` flag. Rolling (`latest-beta` / `latest-stable`) and versioned (`v<X.Y.Z>[-beta]`) tags must both keep being published.
- macOS assets **must** be signed: `rupu-update`'s `verify.rs` runs `codesign --verify --strict` and refuses to swap in an unsigned binary. An unsigned macOS release would break self-update for every existing user.
- `rupu-app` is macOS-only; every build/test command is scoped `-p rupu-cli` or `--workspace --exclude rupu-app`.
- Never run `cargo fmt` package-wide. Format only files you touched.
- Every change goes through a feature branch and a PR.

---

### Task 1: Repository secrets for the macOS job

**This task is the human's, and it blocks Task 4 only.** Tasks 2, 3, 5 and 6 proceed without it.

Six secrets. The Developer ID certificate proves *who built it*; the App Store Connect API key authenticates *notarization submissions*. They are different credentials and both are required.

**Files:** none — GitHub repository settings.

- [ ] **Step 1: Export the Developer ID Application certificate**

In Keychain Access, find **Developer ID Application: Section 9 Labs LLC (995PCLM9KH)**, right-click → Export, choose `.p12`, and set a strong password. Then:

```bash
base64 -i ~/Downloads/developer-id.p12 | pbcopy
```

- `APPLE_CERT_P12_BASE64` — paste the clipboard
- `APPLE_CERT_PASSWORD` — the password you set on export
- `APPLE_TEAM_ID` — `995PCLM9KH`

Delete the `.p12` from Downloads afterwards; it is a signing identity.

- [ ] **Step 2: Create an App Store Connect API key for notarization**

At appstoreconnect.apple.com → Users and Access → Integrations → App Store Connect API, create a key with the **Developer** role. The `.p8` downloads **once**.

```bash
base64 -i ~/Downloads/AuthKey_XXXXXXXXXX.p8 | pbcopy
```

- `APPLE_API_KEY_BASE64` — paste the clipboard
- `APPLE_API_KEY_ID` — the Key ID shown next to the key (10 chars)
- `APPLE_API_ISSUER_ID` — the Issuer ID at the top of that page (a UUID)

An API key is used rather than an Apple ID + app-specific password because it is scoped, revocable, and does not carry a human account's 2FA.

- [ ] **Step 3: Add all six at Settings → Secrets and variables → Actions**

Public-repo secrets are **not** exposed to workflows triggered by fork pull requests, so this is safe on a public repository.

- [ ] **Step 4: Verify they are present**

```bash
gh secret list --repo Section9Labs/rupu
```
Expected: all six listed. Values are never readable back — only overwritable.

---

### Task 2: The `web` job — build the CP UI once

Every binary must embed the *same* UI. Building it per-job invites three subtly different bundles; building it once and sharing the artifact makes that structurally impossible.

**Files:**
- Create: `.github/workflows/release.yml` (the `web` job only; later tasks add the rest)

**Interfaces:**
- Produces: an artifact named `web-dist` containing `crates/rupu-cp/web/dist/`, consumed by all three build jobs.

- [ ] **Step 1: Write the trigger and the web job**

```yaml
name: release

on:
  push:
    tags: ["v*"]
  workflow_dispatch:
    inputs:
      tag:
        description: "Existing tag to (re)publish, e.g. v0.70.3-beta"
        required: true

concurrency:
  group: release-${{ github.ref }}
  cancel-in-progress: false

permissions:
  contents: write

jobs:
  web:
    name: build embedded CP UI
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v5
      - uses: actions/setup-node@v4
        with:
          node-version: "22"
      # Built once and shared, so all three binaries embed a byte-identical
      # UI. Skipping this entirely is what makes rupu-cp fall back to its
      # "not built" placeholder — see Plan 2's gate, which does exactly that
      # deliberately.
      - run: make cp-web
      - uses: actions/upload-artifact@v4
        with:
          name: web-dist
          path: crates/rupu-cp/web/dist
          if-no-files-found: error
          retention-days: 1
```

`cancel-in-progress: false` is deliberate: cancelling a release mid-upload is how you get a half-published release.

- [ ] **Step 2: Verify the YAML parses**

Run: `python3 -c "import yaml; yaml.safe_load(open('.github/workflows/release.yml')); print('ok')"`
Expected: `ok`

- [ ] **Step 3: Commit**

```bash
git add .github/workflows/release.yml
git commit -m "ci(release): build the embedded CP UI once and share it"
```

---

### Task 3: Linux build jobs

**Files:**
- Modify: `.github/workflows/release.yml`

**Interfaces:**
- Consumes: `web-dist` from Task 2; `docker/linux-build.Dockerfile` from Plan 2.
- Produces: artifacts `asset-linux-x64` and `asset-linux-arm64`, each holding the binary named `rupu-<platform>` plus its `.sha256`.

- [ ] **Step 1: Add the matrixed Linux job**

```yaml
  linux:
    name: build ${{ matrix.platform }}
    needs: web
    strategy:
      fail-fast: false
      matrix:
        include:
          - runner: ubuntu-latest
            platform: linux-x64
          - runner: ubuntu-24.04-arm
            platform: linux-arm64
    runs-on: ${{ matrix.runner }}
    timeout-minutes: 60
    steps:
      - uses: actions/checkout@v5
      - uses: actions/download-artifact@v4
        with:
          name: web-dist
          path: crates/rupu-cp/web/dist
      - name: Build the pinned musl image
        run: docker build -f docker/linux-build.Dockerfile -t rupu-linux-build .
      - name: Build (channel-stamped)
        run: |
          docker run --rm \
            -e RUPU_RELEASE_CHANNEL="${{ needs.meta.outputs.channel }}" \
            -e RUPU_RELEASE_VERSION="${{ needs.meta.outputs.version }}" \
            -v "$PWD":/work -w /work rupu-linux-build \
            cargo build --release -p rupu-cli --target-dir target/linux-musl
      - name: Stage the asset under its canonical name
        run: |
          bin=target/linux-musl/release/rupu
          # The binary names itself. Deriving this any other way is the
          # defect #555 fixed.
          platform=$(docker run --rm -v "$PWD":/work -w /work rupu-linux-build \
            ./"$bin" update --print-platform)
          test "$platform" = "${{ matrix.platform }}" \
            || { echo "::error::binary reports '$platform', matrix says '${{ matrix.platform }}'"; exit 1; }
          mkdir -p dist
          cp "$bin" "dist/rupu-$platform"
          ( cd dist && sha256sum "rupu-$platform" > "rupu-$platform.sha256" )
      - uses: actions/upload-artifact@v4
        with:
          name: asset-${{ matrix.platform }}
          path: dist/
          if-no-files-found: error
```

The `test "$platform" = ...` assertion is the point: it cross-checks the binary's own answer against the matrix label, so a runner/arch mix-up cannot publish a mislabelled asset.

- [ ] **Step 2: Verify the YAML parses, then commit**

```bash
python3 -c "import yaml; yaml.safe_load(open('.github/workflows/release.yml')); print('ok')"
git commit -am "ci(release): build linux-x64 and linux-arm64 on native runners"
```

---

### Task 4: macOS build job — sign and notarize

Notarization enters the release path here for the first time. `scripts/notarize-release.sh` has existed for a while but was never wired into `gh-beta`, so published macOS binaries are signed but **not** notarized today.

**Files:**
- Modify: `.github/workflows/release.yml`

**Interfaces:**
- Consumes: `web-dist`; the six secrets from Task 1.
- Produces: artifact `asset-darwin-arm64`.

- [ ] **Step 1: Add the macOS job**

```yaml
  macos:
    name: build darwin-arm64
    needs: [web, meta]
    runs-on: macos-latest
    timeout-minutes: 60
    steps:
      - uses: actions/checkout@v5
      - uses: actions/download-artifact@v4
        with:
          name: web-dist
          path: crates/rupu-cp/web/dist
      - uses: dtolnay/rust-toolchain@stable
        with:
          toolchain: "1.95"

      # Import the Developer ID cert into a throwaway keychain. A dedicated
      # keychain avoids touching the runner's login keychain and is discarded
      # with the runner.
      - name: Import signing certificate
        env:
          CERT_B64: ${{ secrets.APPLE_CERT_P12_BASE64 }}
          CERT_PASSWORD: ${{ secrets.APPLE_CERT_PASSWORD }}
        run: |
          KEYCHAIN=build.keychain
          KEYCHAIN_PASSWORD=$(uuidgen)
          security create-keychain -p "$KEYCHAIN_PASSWORD" "$KEYCHAIN"
          security default-keychain -s "$KEYCHAIN"
          security unlock-keychain -p "$KEYCHAIN_PASSWORD" "$KEYCHAIN"
          security set-keychain-settings -t 3600 -u "$KEYCHAIN"
          echo "$CERT_B64" | base64 --decode > /tmp/cert.p12
          security import /tmp/cert.p12 -k "$KEYCHAIN" -P "$CERT_PASSWORD" \
            -T /usr/bin/codesign
          rm -f /tmp/cert.p12
          security set-key-partition-list -S apple-tool:,apple:,codesign: \
            -s -k "$KEYCHAIN_PASSWORD" "$KEYCHAIN" >/dev/null
          security find-identity -v -p codesigning

      - name: Build (channel-stamped)
        env:
          RUPU_RELEASE_CHANNEL: ${{ needs.meta.outputs.channel }}
          RUPU_RELEASE_VERSION: ${{ needs.meta.outputs.version }}
        run: cargo build --release -p rupu-cli

      - name: Sign
        run: scripts/sign-dev.sh release

      - name: Notarize
        env:
          APPLE_API_KEY_ID: ${{ secrets.APPLE_API_KEY_ID }}
          APPLE_API_ISSUER_ID: ${{ secrets.APPLE_API_ISSUER_ID }}
          APPLE_API_KEY_BASE64: ${{ secrets.APPLE_API_KEY_BASE64 }}
        run: |
          mkdir -p ~/private_keys
          echo "$APPLE_API_KEY_BASE64" | base64 --decode \
            > ~/private_keys/AuthKey_$APPLE_API_KEY_ID.p8
          ditto -c -k --keepParent target/release/rupu /tmp/rupu.zip
          xcrun notarytool submit /tmp/rupu.zip \
            --key ~/private_keys/AuthKey_$APPLE_API_KEY_ID.p8 \
            --key-id "$APPLE_API_KEY_ID" \
            --issuer "$APPLE_API_ISSUER_ID" \
            --wait
          rm -rf ~/private_keys

      - name: Stage the asset under its canonical name
        run: |
          platform=$(./target/release/rupu update --print-platform)
          test "$platform" = "darwin-arm64" \
            || { echo "::error::binary reports '$platform'"; exit 1; }
          mkdir -p dist
          cp target/release/rupu "dist/rupu-$platform"
          ( cd dist && shasum -a 256 "rupu-$platform" > "rupu-$platform.sha256" )

      - uses: actions/upload-artifact@v4
        with:
          name: asset-darwin-arm64
          path: dist/
          if-no-files-found: error
```

Bare binaries cannot be stapled (`stapler` only handles `.app`/`.pkg`/`.dmg`); the notarization ticket is served online, which is why the submit step has no stapling follow-up. `scripts/notarize-release.sh` documents this.

- [ ] **Step 2: Verify the sha256 sidecar format matches what `rupu update` expects**

The sidecar must contain `<hex>  rupu-<platform>` — two spaces, and the *asset* name, not a path. `sha256sum` on Linux and `shasum -a 256` on macOS both produce this when run with `cd dist` first. Confirm against `crates/rupu-update/src/select.rs`.

- [ ] **Step 3: Commit**

```bash
git commit -am "ci(release): macOS build, signed and notarized in CI"
```

---

### Task 5: `meta` and `publish` jobs — channel derivation and atomic upload

**Files:**
- Modify: `.github/workflows/release.yml`

**Interfaces:**
- `meta` produces outputs `channel` (`beta`|`stable`), `version`, `rolling_tag`, `prerelease`.
- `publish` consumes all three asset artifacts and creates/updates both releases.

- [ ] **Step 1: Add the `meta` job**

```yaml
  meta:
    name: derive channel from tag
    runs-on: ubuntu-latest
    outputs:
      channel: ${{ steps.d.outputs.channel }}
      version: ${{ steps.d.outputs.version }}
      rolling_tag: ${{ steps.d.outputs.rolling_tag }}
      prerelease: ${{ steps.d.outputs.prerelease }}
      tag: ${{ steps.d.outputs.tag }}
    steps:
      - id: d
        run: |
          tag="${{ github.event.inputs.tag || github.ref_name }}"
          case "$tag" in
            v*-beta)
              echo "channel=beta"            >> "$GITHUB_OUTPUT"
              echo "rolling_tag=latest-beta" >> "$GITHUB_OUTPUT"
              echo "prerelease=true"         >> "$GITHUB_OUTPUT" ;;
            v*)
              echo "channel=stable"            >> "$GITHUB_OUTPUT"
              echo "rolling_tag=latest-stable" >> "$GITHUB_OUTPUT"
              echo "prerelease=false"          >> "$GITHUB_OUTPUT" ;;
            *)
              echo "::error::tag '$tag' is not v<X.Y.Z>[-beta]"; exit 1 ;;
          esac
          echo "tag=$tag" >> "$GITHUB_OUTPUT"
          echo "version=${tag#v}" >> "$GITHUB_OUTPUT"
```

This reproduces `scripts/gh-build.sh`'s existing semantics exactly. `rupu update` resolves by semver + the `prerelease` flag, not by tag name, so getting `prerelease` right is what actually matters.

- [ ] **Step 2: Add the `publish` job**

```yaml
  publish:
    name: publish release
    needs: [meta, linux, macos]
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v5
      - uses: actions/download-artifact@v4
        with:
          pattern: asset-*
          merge-multiple: true
          path: dist
      - name: Sanity-check the asset set
        run: |
          ls -la dist
          for p in darwin-arm64 linux-x64 linux-arm64; do
            test -f "dist/rupu-$p"        || { echo "::error::missing rupu-$p"; exit 1; }
            test -f "dist/rupu-$p.sha256" || { echo "::error::missing rupu-$p.sha256"; exit 1; }
          done
          ( cd dist && sha256sum -c ./*.sha256 )
      - name: Publish rolling and versioned releases
        env:
          GH_TOKEN: ${{ github.token }}
        run: |
          set -euo pipefail
          notes="Channel: ${{ needs.meta.outputs.channel }}
          Version: ${{ needs.meta.outputs.version }}
          Commit: ${{ github.sha }}

          | Asset | Platform |
          |---|---|
          | rupu-darwin-arm64 | macOS, Apple Silicon (signed + notarized) |
          | rupu-linux-x64    | Linux x86_64 (static musl) |
          | rupu-linux-arm64  | Linux aarch64 (static musl) |

          Windows: install the Linux binary under WSL2."
          for tag in "${{ needs.meta.outputs.rolling_tag }}" "${{ needs.meta.outputs.tag }}"; do
            if gh release view "$tag" >/dev/null 2>&1; then
              gh release edit "$tag" --notes "$notes" \
                --prerelease="${{ needs.meta.outputs.prerelease }}"
            else
              gh release create "$tag" --title "rupu $tag" --notes "$notes" \
                --prerelease="${{ needs.meta.outputs.prerelease }}"
            fi
            gh release upload "$tag" dist/* --clobber
          done
```

Uploading from one job after **all** builds succeed is what makes the release atomic: a failed Linux build means no release at all, rather than a macOS-only release that looks complete.

The rolling tag must also be *moved* to this commit — check whether `gh release create` on an existing rolling tag suffices, or whether an explicit `git tag -f` + push is still needed as `scripts/gh-build.sh` does.

- [ ] **Step 3: Verify the YAML parses and commit**

---

### Task 6: `install.sh`, docs, and retiring the local publish path

**Files:**
- Create: `install.sh`
- Modify: `README.md` (install section), `docs/RELEASING.md`, `Makefile` (`gh-beta` / `gh-stable`)

- [ ] **Step 1: Write `install.sh`**

Detects OS/arch, maps to the same `<os>-<arch>` names, downloads from the rolling tag for a channel (default `stable`), **verifies the sha256**, and installs to `/usr/local/bin`. It must refuse to install on checksum mismatch — a downloader that skips verification is worse than none, since it looks trustworthy.

- [ ] **Step 2: Rewrite the README install section**

A platform table plus the `install.sh` one-liner, and an explicit WSL2 note for Windows — stated, not implied. Replace the current cargo-install-only instructions.

- [ ] **Step 3: Rewrite `docs/RELEASING.md`**

New flow: `make bump VERSION=X.Y.Z` → push the tag → CI publishes everything. Keep `scripts/gh-build.sh` documented as the break-glass path for a darwin-only publish when CI is unavailable.

- [ ] **Step 4: Turn `make gh-beta` / `gh-stable` into tag pushers**

They currently build, sign, and publish locally. They become thin wrappers that push the tag and point at the Actions run. Keep the old behavior reachable under a clearly-named target (e.g. `gh-beta-local`) rather than deleting it.

- [ ] **Step 5: End-to-end verification**

Cut a real beta and confirm:
- three binaries plus three `.sha256` on both the rolling and versioned releases
- `rupu update --check` from an older build finds it
- `install.sh` fetches and verifies on a Linux box
- the macOS asset passes `codesign --verify --strict` **and** `spctl -a -vv -t install`

---

## Self-Review

**Spec coverage.** §4: trigger and channel derivation → Task 5; `web` artifact job → Task 2; Linux jobs → Task 3; macOS cert import/sign/notarize → Tasks 1, 4; atomic publish → Task 5; `make gh-*` become tag pushers → Task 6; secrets list → Task 1. §6: `install.sh`, README, `docs/RELEASING.md` → Task 6.

**Ordering.** Task 1 is the human's and blocks only Task 4. Tasks 2, 3, 5 and 6 are independent of it, so the Linux half can be built and reviewed while the secrets are being generated.

**Known risks.**
- `ubuntu-24.04-arm` is a newer runner label; if unavailable on this account, the fallback is dropping `linux-arm64` initially rather than emulating, which would be slow enough to hit the timeout.
- Notarization is the slowest step and occasionally queues for several minutes; `--wait` handles it, but the 60-minute job timeout is the real bound.
- The rolling-tag move needs confirming (Task 5 Step 2) — `gh release create` against an existing tag does not by itself repoint the tag at a new commit.
