# rupu.app macOS Phase 7 — Ship Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** rupu.app ships from the existing release lane — signed, notarized, stapled, in a DMG + zip on every `v*` release.

**Architecture:** `make macos-release` (Release config, version-stamped, hardened runtime) → `scripts/package-app-dmg.sh` (hdiutil) → a `macos-app` job in `release.yml` reusing the CLI's already-provisioned signing/notarization secrets → assets ride the existing `publish` job. Plus the post-parity backlog doc.

**Tech Stack:** xcodegen/xcodebuild, codesign/notarytool/stapler, hdiutil, GitHub Actions.

**Spec:** `docs/superpowers/specs/2026-08-25-rupu-macos-phase-7-ship-design.md`

## Global Constraints

- No new secrets: consume ONLY the existing `APPLE_CERT_P12_BASE64`/`APPLE_CERT_PASSWORD`/`APPLE_API_KEY_ID`/`APPLE_API_ISSUER_ID`/`APPLE_API_KEY_BASE64`. CI never ships an unsigned/ad-hoc asset — a missing capability fails the job loudly.
- The PR gate (`ci.yml`'s macos-app job) is untouched.
- Workflow edits follow `release.yml`'s existing idioms exactly (tag-pinned checkout with the same doc comment rationale, keychain-import step shape, asset naming/sha256 conventions). Read the whole `macos` job before writing the new one.
- `make macos-test` + `make macos-build` stay green after every task (project.yml changes affect the dev build too).
- Truthful comments; per-file rustfmt (no Rust expected); no third-party deps (no create-dmg, no Sparkle).
- Implementers never dispatch subagents. Never bare `git stash`/`git stash pop`.

---

### Task 1: Release build target + hardened runtime

**Files:**
- Modify: `apps/rupu-macos/project.yml` (hardened runtime ON; version-stamp settings)
- Modify: `Makefile` (new `macos-release` target near the existing macos-* targets)

**Interfaces:**
- Produces: `make macos-release` → `apps/rupu-macos/DerivedData/Build/Products/Release/rupu.app`, with `CFBundleShortVersionString` = `RUPU_RELEASE_VERSION` (env; default `0.0.0-dev`) and `CFBundleVersion` monotonic-friendly (same value is acceptable — doc-comment). Ad-hoc signing locally (`CODE_SIGN_IDENTITY="-"` passed at the xcodebuild invocation in the Make target — CI overrides by re-signing afterward, so the project file itself stays identity-less).

**Steps:**

- [ ] **Step 1:** In `project.yml`: flip `ENABLE_HARDENED_RUNTIME: YES` (the seam is pre-marked "signing/hardening is Phase 7"); route the version into Info.plist — xcodegen supports `settings` with `MARKETING_VERSION`/`CURRENT_PROJECT_VERSION` and Info properties `CFBundleShortVersionString: $(MARKETING_VERSION)` — with the Make target passing `MARKETING_VERSION=${RUPU_RELEASE_VERSION:-0.0.0-dev}` via xcodebuild. Verify the dev build (`make macos-build` + full `make macos-test`) still passes with hardened runtime on — if the Debug run hits a hardened-runtime restriction (it should not; the app neither JITs nor injects), scope the flag to Release via a configs block and record why.
- [ ] **Step 2:** Add `macos-release` to the Makefile: `macos-gen` then `xcodebuild -project apps/rupu-macos/rupu.xcodeproj -scheme rupu -configuration Release -derivedDataPath apps/rupu-macos/DerivedData build CODE_SIGN_IDENTITY=- MARKETING_VERSION=...`. Echo the produced path and stamped version at the end.
- [ ] **Step 3:** Run `make macos-release`; verify with `defaults read` (or `/usr/libexec/PlistBuddy -c Print`) that the built Info.plist carries the stamped version, and `codesign -dv` shows ad-hoc. Run `RUPU_RELEASE_VERSION=9.9.9 make macos-release` and verify 9.9.9 lands.
- [ ] **Step 4:** Full `make macos-test` + `make macos-build` green.
- [ ] **Step 5:** Commit: `build(macos): make macos-release — hardened runtime, version stamping, ad-hoc local signing`

---

### Task 2: DMG + zip packaging script

**Files:**
- Create: `scripts/package-app-dmg.sh`

**Interfaces:**
- Consumes: a built `rupu.app` path (arg 1), output dir (arg 2, default `dist/`), version (env `RUPU_RELEASE_VERSION` for naming only — names stay platform-canonical per the release lane: `rupu-app-darwin-arm64.dmg` / `.zip`).
- Produces: `dist/rupu-app-darwin-arm64.dmg` (hdiutil UDZO; staging dir with the .app + an `/Applications` symlink; volume name "rupu"), `dist/rupu-app-darwin-arm64.zip` (`ditto -c -k --keepParent`), and `.sha256` files for both (same `shasum -a 256` convention as the CLI assets).

**Steps:**

- [ ] **Step 1:** Write the script following `scripts/sign-dev.sh`'s conventions (bash, `set -euo pipefail`, non-macOS no-op with message, usage comment). Staging in a `mktemp -d` cleaned by trap.
- [ ] **Step 2:** Run it against the Task 1 build; verify: `hdiutil attach`-able DMG containing `rupu.app` + `Applications` symlink; zip round-trips (`ditto -x -k`) to a launchable app; sha256 files match.
- [ ] **Step 3:** Full `make macos-test` green (unchanged code, gate discipline).
- [ ] **Step 4:** Commit: `build(macos): package-app-dmg.sh — hdiutil DMG + zip + checksums`

---

### Task 3: release.yml macos-app job

**Files:**
- Modify: `.github/workflows/release.yml`

**Interfaces:**
- Consumes: the existing `meta` job outputs (tag/version/channel), the keychain-import step (copy its exact shape — the doc comments included), the notarize secrets, `make macos-release`, `scripts/package-app-dmg.sh`.
- Produces: job `macos-app` (name "build rupu.app darwin-arm64", `needs: [meta]`, macos-latest, tag-pinned checkout with the same rationale comment as the CLI macos job): xcodegen install (check how CI's `ci.yml` macos-app job installs xcodegen and mirror it) → `make macos-release` with `RUPU_RELEASE_VERSION` → **re-sign** the app bundle with the Developer ID identity (`codesign --force --deep --options runtime --timestamp -s "$IDENTITY" rupu.app` — deep-sign rationale doc-commented; find the identity the same way sign-dev.sh does) → `ditto` zip → `notarytool submit --wait` (same key plumbing as the CLI job) → `xcrun stapler staple rupu.app` (apps staple, unlike the CLI's bare binary — cite the existing comment) → repackage via `package-app-dmg.sh` (the DMG contains the STAPLED app) → sign + notarize + staple the DMG itself → upload artifact `asset-app-darwin-arm64`.
- `publish` job: add `macos-app` to `needs` and its `dist/` merge (read how the other asset artifacts flow into the release upload and mirror exactly).
- Dry-run affordance (spec §5): `release.yml` is triggered by tag push/dispatch — add an optional `workflow_dispatch` input `app_dry_run` that, when true, runs ONLY meta+web+macos-app without uploading (skip publish via an `if:`). Keep the conditional surface minimal; doc-comment it.

**Steps:**

- [ ] **Step 1:** Read the ENTIRE existing `macos` job + `publish` job + `meta` outputs + ci.yml's xcodegen install step. Write the new job.
- [ ] **Step 2:** Validate: `actionlint` if installed, else `python3 -c "import yaml,sys; yaml.safe_load(open('.github/workflows/release.yml'))"` parse + `gh workflow view` after push (the latter lands at PR time). Grep-audit that every secret referenced exists in the current file already (no new names).
- [ ] **Step 3:** Full `make macos-test` green (gate discipline).
- [ ] **Step 4:** Commit: `ci(macos): release lane ships rupu.app — sign, notarize, staple, DMG+zip`

---

### Task 4: Docs + post-parity backlog

**Files:**
- Modify: `README.md` (install section for rupu.app: DMG download, drag-to-Applications, requires an installed `rupu` ≥ the app's VersionGate floor — read `RupuBackend`'s `VersionGate.minimum` and cite the actual value)
- Modify: `docs/superpowers/specs/2026-08-20-rupu-macos-app-design.md` (§8 Phase 7 row → delivered)
- Create: `docs/superpowers/specs/2026-08-25-rupu-macos-post-parity-backlog.md`
- Modify: `CLAUDE.md` (Read-first: Phase 7 spec/plan lines; module-map note that release packaging exists)

**Interfaces:**
- The backlog doc: one table (item · origin phase · disposition source · notes) collecting every deferred-tracked item named in the umbrella §8 rows and phase specs §"deferred"/"out of scope" sections for Phases 2–6, plus the parked visual nits listed in the spec (§4 list). Every row must cite where the deferral was recorded (spec/plan section) — no invented items, no dropped items: cross-check against the umbrella rows for phases 2–6 and the Phase 5/6 spec deferral sections.

**Steps:**

- [ ] **Step 1:** Sweep the umbrella §8 rows + phase specs for deferral language; build the table; write the doc.
- [ ] **Step 2:** README section + umbrella row + CLAUDE.md lines.
- [ ] **Step 3:** Full `make macos-test` green.
- [ ] **Step 4:** Commit: `docs(macos): Phase 7 ship docs — install guide, post-parity backlog seeded`

---

## Post-plan (controller)

Final whole-branch review, PR, merge on green. The sign→notarize→staple path is proven by cutting a beta (`gh workflow run release-beta.yml`) at the checkpoint — per matt's standing "the usual ends in a pushed beta" — with the checkpoint notes stating exactly what remained unproven before that cut. NOTE: release-beta targets the CLI cadence; confirm whether the beta path exercises release.yml's new job (it must — beta releases go through release.yml on the tag push) before claiming the app shipped.
