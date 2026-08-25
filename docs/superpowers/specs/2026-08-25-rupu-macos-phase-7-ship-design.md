# rupu.app macOS — Phase 7: Ship

**Date:** 2026-08-25
**Status:** Executed under matt's standing "continue with all of the phases" authorization; decisions recorded for the phase checkpoint
**Parent:** `docs/superpowers/specs/2026-08-20-rupu-macos-app-design.md` (umbrella §8, Phase 7)
**Plan:** `docs/superpowers/plans/2026-08-25-rupu-macos-phase-7-ship.md`

The distribution phase. One plan. The heavy lifting already exists: the CLI's release lane (`.github/workflows/release.yml`, `macos` job) imports a Developer ID certificate from provisioned secrets (`APPLE_CERT_P12_BASE64`/`APPLE_CERT_PASSWORD`), signs via `scripts/sign-dev.sh`, and notarizes via `notarytool` with App Store Connect API secrets (`APPLE_API_KEY_ID`/`APPLE_API_ISSUER_ID`/`APPLE_API_KEY_BASE64`). Phase 7 extends that lane to the app — it does NOT build new signing infrastructure.

## 1. Release build

A `make macos-release` target: `xcodegen` + `xcodebuild -configuration Release` producing `rupu.app` with version stamping — `CFBundleShortVersionString`/`CFBundleVersion` injected from `RUPU_RELEASE_VERSION` (the same env the CLI build consumes; default `0.0.0-dev` locally). Hardened runtime enabled in `project.yml` (a notarization requirement); entitlements limited to what the app actually uses (unsandboxed, matching the current dev build — sandboxing is out of scope and recorded). Local runs without a Developer ID cert sign ad-hoc (`CODE_SIGN_IDENTITY=-`), loudly labeled.

## 2. CI release job

A `macos-app` job in `release.yml` (needs: meta, runs on the tag ref like the CLI's `macos` job): build via `make macos-release`, sign the bundle with the same imported Developer ID identity (deep, hardened runtime, timestamp), notarize the zipped `.app` with the same notarytool secrets, **staple** the ticket (apps staple; bare binaries don't — the CLI job's comment already documents that asymmetry), then package and upload. The job reuses the existing keychain-import step verbatim (extract it into a composite/step-anchor only if the duplication is textually large — reviewer's call). Release assets: `rupu-app-darwin-arm64.dmg` + `.zip` + `.sha256` files, uploaded alongside the CLI assets by the existing `publish` job (its `needs` list grows). The app ships on the same `v*` tag cadence and channels (beta soak → stable promote) as the CLI — no separate release train.

## 3. DMG

`scripts/package-app-dmg.sh`: `hdiutil`-based (no third-party tooling, matching the no-deps rule) — staging dir with `rupu.app` + `/Applications` symlink, `hdiutil create -format UDZO`. The DMG itself is signed and notarized (stapled), so first-launch passes Gatekeeper cleanly.

## 4. Docs + backlog seeding

- README gains an install section for rupu.app (DMG download, drag to Applications, server-version floor: the app's `VersionGate.minimum` — currently `0.74.0` — must be stated; the app attach-or-spawns a local `rupu` from PATH).
- Umbrella §8 Phase 7 row marked delivered.
- **Post-parity backlog seeded**: one doc (`docs/superpowers/specs/2026-08-25-rupu-macos-post-parity-backlog.md`) collecting every deferred-tracked item recorded across Phases 2–6 (typed config forms; critical-finding notifications; fs/browse; coverage audit/gap/diff/templates; agent/session-surface finding navigation; custom usage windows; SR follow/pin + fresh-highlight; claims manual refresh; richer finding rows; definition editors + "Used by"; add-host forms; offset-keyed sortable tables + the parked visual nits from the redesign memory) — each with its origin phase and disposition, so the redesign pass and post-parity work start from one honest list.

## 5. Constraints

- No new secrets: the job must consume ONLY the already-provisioned `APPLE_*` secrets. If a needed capability is missing from them (e.g. the cert lacks Developer ID Application usage for bundles), the job fails loudly at the sign step — never falls back to shipping an unsigned or ad-hoc-signed asset from CI (self-update-adjacent honesty; users' Gatekeeper trust is the product).
- CI's PR job (`macos-app` in `ci.yml`) is untouched — Debug build + tests remain the PR gate.
- The app has no self-update mechanism this phase (it is a CP client updated by downloading a new DMG); recorded, not built. Sparkle/appcast is post-parity.
- The release job must be testable without cutting a release: a `workflow_dispatch` dry-run input that builds/signs/notarizes but skips upload, OR validation via the beta channel's next cut — the plan picks the cheaper honest option.

## 6. Verification

`make macos-release` builds locally (ad-hoc). Workflow YAML validated (actionlint if available, else `gh workflow view` parse). The full sign→notarize→staple→DMG path can only be proven by a real CI run with secrets — the plan's final step cuts a beta (`gh workflow run release-beta.yml`) per matt's "do the usual ends in a pushed beta" convention ONLY at the phase checkpoint with the merge, and the checkpoint notes state exactly which steps ran unproven until then.
