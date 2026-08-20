# rupu.app macOS — Phase 1: Foundation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stand up the native SwiftUI app skeleton — scaffold, design tokens, window shell + routing, backend manager (attach-or-spawn), CP API client + SSE + golden-fixture rig, onboarding — and delete the GPUI `rupu-app` crate.

**Architecture:** One thin app target + a local Swift Package (`RupuKit`) with modules `RupuAPI` / `RupuBackend` / `RupuStore` / `RupuDesign` / `RupuShell`. The Rust side is reached only via the CP HTTP/SSE API. Golden JSON fixtures emitted by rupu-cp tests are decoded by RupuAPI tests, so serde drift breaks CI.

**Tech Stack:** SwiftUI (macOS 14+), Swift 6 (`swift-tools-version: 6.0`), XcodeGen, `URLSession` (REST + SSE), Security.framework Keychain. No third-party Swift dependencies.

**Spec:** `docs/superpowers/specs/2026-08-20-rupu-macos-app-design.md` (umbrella). Visual contract: `docs/macOS_design/HANDOFF.md`.

## Global Constraints

- Deployment target **macOS 14.0**; Swift tools version **6.0**.
- **No third-party Swift dependencies** (spec §3).
- **Thin app target**: `App/` contains only `@main`, scene declarations, and wiring. All logic/views live in RupuKit modules (spec §3).
- `.xcodeproj` is **gitignored**; `project.yml` is the only committed project definition.
- **No silent-noop UI**: a control that isn't wired to a real endpoint does not render (spec §9). Placeholder screens show explicit "not built yet" empty states, never fake data or dead buttons.
- Null discipline: unknown values render `—`, never `0` (spec §9).
- Rust side: workspace deps only; `#![deny(clippy::all)]`; **never run package-wide `cargo fmt`** (main is fmt-dirty under the pinned toolchain — format only files you created/edited, via `rustfmt <file>`).
- Never run bare `git stash pop` (shared stash across sessions).
- Commit per task; the whole plan lands as PRs off `main` (Task 1 is its own PR; Tasks 2–10 are the foundation PR).
- GUI validation rule: build+test green ≠ rendering green. Final validation = computer-use screenshots + matt runs the app before merge (spec §9).
- Default embedded CP port: **7420** (HANDOFF §Stack). Minimum supported rupu version: **0.71.0**.

---

### Task 1: Delete the GPUI rupu-app crate

Its value (graph view, launcher, approvals) is re-delivered by Phases 2–4. `rupu-app-canvas` **stays** — `rupu-cli` consumes it (`crates/rupu-cli/src/cmd/workflow.rs`, `crates/rupu-cli/src/output/live_run.rs`). This task is an independent PR.

**Files:**
- Delete: `crates/rupu-app/` (entire directory)
- Modify: `Cargo.toml` (workspace members — remove `"crates/rupu-app"`, keep `"crates/rupu-app-canvas"`)
- Modify: `.github/workflows/ci.yml:212-221` and `:283` (remove the `--exclude rupu-app` flags and the macOS-only explanatory comment)
- Modify: `Makefile` (delete `app-smoke` and `app-run` targets and their `help` lines; keep `cargo test -p rupu-scm --test clone` coverage — it already runs under `make test`'s workspace `cargo test`)
- Modify: `CLAUDE.md` (delete the "rupu-app rules" section; delete the `rupu-app` crate bullet; in the `rupu-app-canvas` bullet, replace "rupu-app's `view/graph.rs` consumes the rows and paints with GPUI text spans" with "consumed by `rupu-cli`'s workflow output (`cmd/workflow.rs`, `output/live_run.rs`)" and drop the D-6 sentence)

**Interfaces:**
- Consumes: nothing.
- Produces: a workspace with no GPUI dependency; CI commands run without `--exclude rupu-app`.

- [ ] **Step 1: Create branch** `git checkout main && git pull && git checkout -b chore/delete-rupu-app`
- [ ] **Step 2: Verify no other consumers of rupu-app** — Run: `grep -rn "rupu-app\"" crates/*/Cargo.toml | grep -v rupu-app-canvas` and `grep -rln "rupu_app::" crates --include='*.rs' | grep -v "crates/rupu-app/"`. Expected: no output. If anything appears, STOP and report — the spec's deletion premise is wrong.
- [ ] **Step 3: Delete** — `git rm -r crates/rupu-app` and remove the `"crates/rupu-app",` member line from root `Cargo.toml`. Also remove any `[workspace.dependencies]` entries used *only* by rupu-app (check each candidate with `grep -rn "<dep-name>" crates/*/Cargo.toml`); leave shared ones.
- [ ] **Step 4: Edit ci.yml** — change `cargo test --workspace --exclude rupu-app --locked` → `cargo test --workspace --locked`, and `cargo clippy --workspace --exclude rupu-app --all-targets --locked -- -D warnings` → `cargo clippy --workspace --all-targets --locked -- -D warnings`; delete the stale comment block at ci.yml:212.
- [ ] **Step 5: Edit Makefile + CLAUDE.md** per the Files list above.
- [ ] **Step 6: Verify** — Run: `cargo build --workspace && make lint && make test`. Expected: green (rupu-app-canvas still builds; its insta tests still pass). `Cargo.lock` will shrink — commit the change.
- [ ] **Step 7: Commit & PR** — `git add -A && git commit -m "chore: delete GPUI rupu-app (superseded by SwiftUI rupu.app — see 2026-08-20 spec)"`, push with explicit refspec, open PR.

---

### Task 2: XcodeGen scaffold + Make targets

**Files:**
- Create: `apps/rupu-macos/project.yml`
- Create: `apps/rupu-macos/App/RupuApp.swift`, `apps/rupu-macos/App/Info.plist`
- Create: `apps/rupu-macos/RupuKit/Package.swift` + one placeholder source per module + one passing test per test target
- Create: `apps/rupu-macos/Fixtures/.gitkeep`
- Modify: `.gitignore`, `Makefile`

**Interfaces:**
- Produces: module names `RupuAPI`, `RupuBackend`, `RupuStore`, `RupuDesign`, `RupuShell` (library product `RupuKit`); Make targets `macos-gen`, `macos-build`, `macos-test`; app target named `rupu`, bundle id `com.section9labs.rupu`.

- [ ] **Step 1: Branch** — `git checkout main && git pull && git checkout -b feat/macos-foundation` (Tasks 2–10 all land here).
- [ ] **Step 2: Write `apps/rupu-macos/RupuKit/Package.swift`:**

```swift
// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "RupuKit",
    platforms: [.macOS(.v14)],
    products: [
        .library(
            name: "RupuKit",
            targets: ["RupuAPI", "RupuBackend", "RupuStore", "RupuDesign", "RupuShell"]
        )
    ],
    targets: [
        .target(name: "RupuAPI"),
        .target(name: "RupuBackend", dependencies: ["RupuAPI"]),
        .target(name: "RupuDesign"),
        .target(name: "RupuStore", dependencies: ["RupuAPI", "RupuBackend"]),
        .target(name: "RupuShell", dependencies: ["RupuAPI", "RupuBackend", "RupuStore", "RupuDesign"]),
        .testTarget(name: "RupuAPITests", dependencies: ["RupuAPI"]),
        .testTarget(name: "RupuBackendTests", dependencies: ["RupuBackend"]),
        .testTarget(name: "RupuDesignTests", dependencies: ["RupuDesign"]),
        .testTarget(name: "RupuStoreTests", dependencies: ["RupuStore"]),
    ]
)
```

Each `Sources/<Module>/<Module>.swift` placeholder is an empty `public enum <Module>Module {}` (deleted as real code arrives); each test target gets one trivially-passing test so `swift test` exercises every target from day one.

- [ ] **Step 3: Write `apps/rupu-macos/project.yml`:**

```yaml
name: rupu
options:
  bundleIdPrefix: com.section9labs
  deploymentTarget:
    macOS: "14.0"
  createIntermediateGroups: true
packages:
  RupuKit:
    path: RupuKit
targets:
  rupu:
    type: application
    platform: macOS
    sources: [App]
    dependencies:
      - package: RupuKit
        product: RupuKit
    info:
      path: App/Info.plist
      properties:
        CFBundleDisplayName: rupu
        LSMinimumSystemVersion: "14.0"
        NSHumanReadableCopyright: "© Section9Labs"
    settings:
      base:
        PRODUCT_BUNDLE_IDENTIFIER: com.section9labs.rupu
        SWIFT_VERSION: "6.0"
        ENABLE_HARDENED_RUNTIME: NO   # signing/hardening is Phase 7
        ENABLE_APP_SANDBOX: NO        # spawns the installed rupu binary
```

- [ ] **Step 4: Write `App/RupuApp.swift`** (thin; grows scenes in Tasks 8–9):

```swift
import SwiftUI
import RupuShell

@main
struct RupuApp: App {
    var body: some Scene {
        WindowGroup {
            Text("rupu — foundation scaffold")
                .frame(minWidth: 1150, minHeight: 760)
        }
    }
}
```

- [ ] **Step 5: `.gitignore` additions:**

```
apps/rupu-macos/*.xcodeproj
apps/rupu-macos/RupuKit/.build/
apps/rupu-macos/DerivedData/
.swiftpm/
```

- [ ] **Step 6: Makefile targets** (+ matching `help` lines):

```make
macos-gen:
	xcodegen generate --spec apps/rupu-macos/project.yml

macos-build: macos-gen
	xcodebuild -project apps/rupu-macos/rupu.xcodeproj -scheme rupu \
		-configuration Debug -derivedDataPath apps/rupu-macos/DerivedData \
		CODE_SIGNING_ALLOWED=NO build

macos-test:
	swift test --package-path apps/rupu-macos/RupuKit

macos-run: macos-build
	open apps/rupu-macos/DerivedData/Build/Products/Debug/rupu.app
```

- [ ] **Step 7: Verify** — Run: `make macos-test` (all placeholder tests pass) and `make macos-build` (BUILD SUCCEEDED). Run `make macos-run` and confirm a window opens.
- [ ] **Step 8: Commit** — `git add apps/rupu-macos .gitignore Makefile && git commit -m "feat(macos): XcodeGen scaffold, RupuKit package, make targets"`

---

### Task 3: RupuDesign — tokens, type styles, formatters

Deviation from HANDOFF (agreed at design time): tokens are **programmatic dynamic colors**, not an asset catalog — identical rendering, diffable source.

**Files:**
- Create: `RupuKit/Sources/RupuDesign/Tokens.swift`, `Typography.swift`, `Formatters.swift`
- Test: `RupuKit/Tests/RupuDesignTests/FormattersTests.swift`, `TokensTests.swift`

**Interfaces:**
- Produces: `Color.rupuBg/.rupuPanel/.rupuSurface/.rupuHover/.rupuActive/.rupuBorder/.rupuBorderStrong/.rupuInk/.rupuDim/.rupuMute/.rupuBrand/.rupuBrandHi`; `Color.status(RunTone)` with `public enum RunTone { case run, done, fail, await, pause }`; `Color.severity(Severity)` with `public enum Severity: String, CaseIterable { case crit, high, med, low, info }`; `Font.microLabel` / `Font.identifier` / `Font.numeral(size:)`; `Fmt.count(_ n: Int?) -> String`, `Fmt.partial(_ n: Int, isPartial: Bool) -> String`, `Fmt.duration(ms: UInt64) -> String`.

- [ ] **Step 1: Write failing tests** (`FormattersTests.swift`):

```swift
import Testing
@testable import RupuDesign

@Test func nilCountRendersDash() {
    #expect(Fmt.count(nil) == "—")
    #expect(Fmt.count(0) == "0")
    #expect(Fmt.count(1234) == "1,234")
}
@Test func partialSumsAreMarked() {
    #expect(Fmt.partial(12, isPartial: true) == "12+")
    #expect(Fmt.partial(12, isPartial: false) == "12")
}
@Test func durations() {
    #expect(Fmt.duration(ms: 850) == "0.9s")
    #expect(Fmt.duration(ms: 4_200) == "4.2s")
    #expect(Fmt.duration(ms: 72_000) == "1m 12s")
    #expect(Fmt.duration(ms: 3_720_000) == "1h 2m")
}
```

And `TokensTests.swift` — resolve each dynamic NSColor under `.aqua` and `.darkAqua` appearances and assert the hex from HANDOFF's table (e.g. bg → `#FAFAFA` light / `#0A0A0A` dark; use `NSAppearance(named:)!.performAsCurrentDrawingAppearance` + `usingColorSpace(.sRGB)` and compare rounded 8-bit components).

- [ ] **Step 2: Run** `make macos-test` — Expected: FAIL (Fmt/tokens undefined).
- [ ] **Step 3: Implement `Tokens.swift`:**

```swift
import SwiftUI
import AppKit

func dynamicColor(light: UInt32, dark: UInt32) -> Color {
    Color(nsColor: NSColor(name: nil) { appearance in
        let isDark = appearance.bestMatch(from: [.aqua, .darkAqua]) == .darkAqua
        return NSColor(srgbHex: isDark ? dark : light)
    })
}

extension NSColor {
    convenience init(srgbHex hex: UInt32) {
        self.init(srgbRed: CGFloat((hex >> 16) & 0xFF) / 255,
                  green: CGFloat((hex >> 8) & 0xFF) / 255,
                  blue: CGFloat(hex & 0xFF) / 255, alpha: 1)
    }
}

public extension Color {
    static let rupuBg = dynamicColor(light: 0xFAFAFA, dark: 0x0A0A0A)
    static let rupuPanel = dynamicColor(light: 0xFFFFFF, dark: 0x141416)
    static let rupuSurface = dynamicColor(light: 0xF1F5F9, dark: 0x1B1B1F)
    static let rupuHover = dynamicColor(light: 0xE2E8F0, dark: 0x232327)
    static let rupuActive = dynamicColor(light: 0xCBD5E1, dark: 0x2E2E33)
    static let rupuBorder = dynamicColor(light: 0xE5E7EB, dark: 0x26262A)
    static let rupuBorderStrong = dynamicColor(light: 0xCBD5E1, dark: 0x3F3F46) // lines/fills only, never text
    static let rupuInk = dynamicColor(light: 0x0F172A, dark: 0xF5F5F5)
    static let rupuDim = dynamicColor(light: 0x64748B, dark: 0xA1A1AA)
    static let rupuMute = dynamicColor(light: 0x94A3B8, dark: 0x71717A) // dimmest legal text
    static let rupuBrand = dynamicColor(light: 0x7C3AED, dark: 0x7C3AED)
    static let rupuBrandHi = dynamicColor(light: 0x6D28D9, dark: 0xA78BFA)
}

public enum RunTone: String, CaseIterable, Sendable { case run, done, fail, waiting = "await", pause }
public enum Severity: String, CaseIterable, Sendable { case crit, high, med, low, info }

public extension Color {
    static func status(_ tone: RunTone) -> Color {
        switch tone {
        case .run: dynamicColor(light: 0x3B82F6, dark: 0x60A5FA)
        case .done: dynamicColor(light: 0x16A34A, dark: 0x4ADE80)
        case .fail: dynamicColor(light: 0xDC2626, dark: 0xF87171)
        case .waiting: dynamicColor(light: 0xD97706, dark: 0xFBBF24)
        case .pause: dynamicColor(light: 0x0891B2, dark: 0x22D3EE)
        }
    }
    static func severity(_ s: Severity) -> Color {
        switch s {
        case .crit: dynamicColor(light: 0x9333EA, dark: 0xA855F7)
        case .high: dynamicColor(light: 0xDC2626, dark: 0xF87171)
        case .med: dynamicColor(light: 0xEA580C, dark: 0xFB923C)
        case .low: dynamicColor(light: 0xCA8A04, dark: 0xFACC15)
        case .info: dynamicColor(light: 0x64748B, dark: 0x94A3B8)
        }
    }
}
```

- [ ] **Step 4: Implement `Typography.swift`** — `Font.microLabel` = `.system(size: 10, design: .monospaced).weight(.medium)` used with `.textCase(.uppercase)` + `.kerning(1.2)` by a `MicroLabel(_ text:)` view; `Font.identifier` = `.system(size: 11.5, design: .monospaced)`; `Font.numeral(size:)` = `.system(size: size, design: .monospaced).monospacedDigit()`. Also `PanelStyle` ViewModifier: `.background(Color.rupuPanel)`, corner radius 8, 1px `.rupuBorder` stroke, **no shadow** (inner-card variant radius 6).
- [ ] **Step 5: Implement `Formatters.swift`** to satisfy Step 1 exactly (`count` uses `NumberFormatter` grouping; `duration` rounds: `<60s` → one decimal `s`, `<1h` → `Nm Ns`, else `Nh Nm`).
- [ ] **Step 6: Run** `make macos-test` — Expected: PASS.
- [ ] **Step 7: Commit** — `feat(macos): RupuDesign tokens, typography, null-discipline formatters`

---

### Task 4: Golden fixture emitter in rupu-cp

Rust tests serialize the real serde types into `apps/rupu-macos/Fixtures/*.json`. Without `REGEN_FIXTURES=1` they **assert** the checked-in file matches (drift ⇒ CI failure); with it they rewrite.

**Files:**
- Create: `crates/rupu-cp/tests/macos_fixtures.rs`
- Modify: `crates/rupu-cp/src/api/host_info.rs` (add `#[cfg(test)] mod tests` fixture check)
- Create (generated): `apps/rupu-macos/Fixtures/host_info.json`, `apps/rupu-macos/Fixtures/events.json`
- Modify: `Makefile` (add `macos-fixtures` target)

**Interfaces:**
- Consumes: `rupu_orchestrator::executor::Event`, `rupu_orchestrator::runs::{RunStatus, StepKind}` (all pub).
- Produces: `Fixtures/host_info.json` = `{"version": "...", "capabilities": {"backends": [...], "scm_hosts": [...], "permission_modes": [...]}}`; `Fixtures/events.json` = a JSON **array** covering every `Event` variant (tagged `"type"`, snake_case), serialized pretty with stable order.

- [ ] **Step 1: Write the shared helper + event fixture test** (`tests/macos_fixtures.rs`):

```rust
#![deny(clippy::all)]
//! Golden fixtures for the macOS app (apps/rupu-macos/Fixtures/).
//! `cargo test -p rupu-cp --test macos_fixtures` asserts no drift;
//! `REGEN_FIXTURES=1` rewrites. Swift decodes these in RupuAPITests.

use std::path::PathBuf;

use chrono::{TimeZone, Utc};
use rupu_orchestrator::executor::Event;
use rupu_orchestrator::runs::{RunStatus, StepKind};

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../apps/rupu-macos/Fixtures")
}

fn check_fixture(name: &str, value: &impl serde::Serialize) {
    let path = fixtures_dir().join(name);
    let rendered = serde_json::to_string_pretty(value).expect("serialize fixture");
    if std::env::var_os("REGEN_FIXTURES").is_some() {
        std::fs::write(&path, rendered + "\n").expect("write fixture");
        return;
    }
    let on_disk = std::fs::read_to_string(&path)
        .unwrap_or_else(|_| panic!("missing fixture {name}; run `make macos-fixtures`"));
    assert_eq!(
        on_disk.trim_end(),
        rendered,
        "fixture {name} drifted from the Rust types; run `make macos-fixtures` and update the Swift models"
    );
}

#[test]
fn events_fixture_is_current() {
    let t = Utc.with_ymd_and_hms(2026, 8, 20, 12, 0, 0).unwrap();
    let events: Vec<Event> = vec![
        Event::RunStarted { event_version: 1, run_id: "run-01".into(), workflow_path: "wf/x.yaml".into(), started_at: t },
        Event::StepStarted { run_id: "run-01".into(), step_id: "plan".into(), kind: StepKind::Linear, agent: Some("rupuso".into()), host: None },
        Event::StepWorking { run_id: "run-01".into(), step_id: "plan".into(), note: Some("thinking".into()), transcript_path: Some("t/plan.jsonl".into()) },
        Event::StepAwaitingApproval { run_id: "run-01".into(), step_id: "gate".into(), reason: "manual gate".into() },
        Event::StepCompleted { run_id: "run-01".into(), step_id: "plan".into(), success: true, duration_ms: 4200, host: Some("mini".into()) },
        Event::StepFailed { run_id: "run-01".into(), step_id: "build".into(), error: "exit 1".into() },
        Event::StepSkipped { run_id: "run-01".into(), step_id: "deploy".into(), reason: "branch untaken".into() },
        Event::UnitStarted { run_id: "run-01".into(), step_id: "fan".into(), index: 0, unit_key: "crates/a".into(), agent: None, transcript_path: "t/u0.jsonl".into(), host: None },
        Event::UnitCompleted { run_id: "run-01".into(), step_id: "fan".into(), index: 0, unit_key: "crates/a".into(), success: true, tokens_in: 0, tokens_out: 0, host: None },
        Event::PanelRound { run_id: "run-01".into(), step_id: "review".into(), round: 2, max_iterations: 5, max_severity_remaining: Some("high".into()) },
        Event::RunCompleted { run_id: "run-01".into(), status: RunStatus::Completed, finished_at: t },
        Event::RunFailed { run_id: "run-01".into(), error: "step build failed".into(), finished_at: t },
        Event::RunPaused { run_id: "run-01".into() },
        Event::RunResumed { run_id: "run-01".into() },
        Event::StepPaused { run_id: "run-01".into(), step_id: "plan".into() },
        Event::StepResumed { run_id: "run-01".into(), step_id: "plan".into() },
        Event::DispatchStarted { run_id: "run-01".into(), sub_run_id: "run-02".into(), agent: Some("reviewer".into()), transcript_path: "t/d.jsonl".into() },
        Event::DispatchCompleted { run_id: "run-01".into(), sub_run_id: "run-02".into(), success: true, tokens_in: 1000, tokens_out: 200 },
    ];
    check_fixture("events.json", &events);
}
```

If any variant's fields don't compile (the enum moved since this plan), fix the constructor to match the real enum — the fixture must construct **every** current variant; add/remove entries accordingly and note it in the commit message.

- [ ] **Step 2: host_info fixture** — in `crates/rupu-cp/src/api/host_info.rs` add:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_info_fixture_is_current() {
        let value = HostInfoResponse {
            version: "0.71.0".to_string(),
            capabilities: HostCapabilities {
                backends: vec!["claude".into(), "codex".into()],
                scm_hosts: vec!["github.com".into()],
                permission_modes: vec!["ask".into(), "bypass".into(), "readonly".into()],
            },
        };
        // Same helper contract as tests/macos_fixtures.rs (duplicated: unit tests
        // can't share code with integration tests without a public module).
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../apps/rupu-macos/Fixtures");
        let path = dir.join("host_info.json");
        let rendered = serde_json::to_string_pretty(&value).expect("serialize");
        if std::env::var_os("REGEN_FIXTURES").is_some() {
            std::fs::write(&path, rendered + "\n").expect("write fixture");
            return;
        }
        let on_disk = std::fs::read_to_string(&path)
            .expect("missing fixture host_info.json; run `make macos-fixtures`");
        assert_eq!(on_disk.trim_end(), rendered, "host_info fixture drifted; run `make macos-fixtures`");
    }
}
```

Note: the fixture `version` is a frozen sample value, deliberately **not** `env!("CARGO_PKG_VERSION")` — fixtures must not churn on every release bump.

- [ ] **Step 3: Makefile target:**

```make
macos-fixtures:
	REGEN_FIXTURES=1 cargo test -p rupu-cp --test macos_fixtures
	REGEN_FIXTURES=1 cargo test -p rupu-cp host_info_fixture_is_current
```

- [ ] **Step 4: Generate + verify** — Run `make macos-fixtures` (writes both files), then `cargo test -p rupu-cp --test macos_fixtures && cargo test -p rupu-cp host_info_fixture_is_current` **without** the env var. Expected: PASS. Inspect `apps/rupu-macos/Fixtures/*.json` — events.json entries must each carry `"type": "<snake_case>"`.
- [ ] **Step 5: rustfmt only the touched files** — `rustfmt crates/rupu-cp/tests/macos_fixtures.rs` (host_info.rs: format only if it was already clean — check `git diff` stays scoped to your edit).
- [ ] **Step 6: Commit** — `feat(macos): golden JSON fixture rig in rupu-cp (host_info + events)`

---

### Task 5: RupuAPI — models + CPClient

**Files:**
- Create: `RupuKit/Sources/RupuAPI/Models.swift`, `CPEvent.swift`, `CPClient.swift`
- Test: `RupuKit/Tests/RupuAPITests/FixtureDecodingTests.swift`, `CPClientTests.swift`
- Create: `RupuKit/Tests/RupuAPITests/FixtureLoader.swift`

**Interfaces:**
- Consumes: `apps/rupu-macos/Fixtures/*.json` (Task 4).
- Produces:
  - `public struct HostInfo: Decodable, Equatable, Sendable { public let version: String; public let capabilities: HostCapabilities }`
  - `public struct HostCapabilities: Decodable, Equatable, Sendable { public let backends: [String]; public let scmHosts: [String]; public let permissionModes: [String] }`
  - `public enum CPEvent: Equatable, Sendable, Decodable` (variants below) with `public var runID: String?`
  - `public struct CPConfig: Sendable { public var baseURL: URL; public var token: String?; public init(baseURL: URL, token: String? = nil) }`
  - `public actor CPClient { public init(config: CPConfig, session: URLSession = .shared); public func hostInfo() async throws -> HostInfo; public func recentEvents(limit: Int) async throws -> [CPEventRow] }`
  - `public struct CPEventRow: Decodable, Equatable, Sendable { public let event: CPEvent; public let ts: Int64?; public let pos: Int? }` (decodes ts/pos and the event from the *same* JSON object — `/api/events` injects `ts`/`pos` into the event payload)
  - `public enum CPError: Error, Equatable { case http(status: Int, body: String), transport(String), decoding(String), unauthorized }`

- [ ] **Step 1: Fixture loader** (`FixtureLoader.swift`) — resolves the repo-level Fixtures dir from the test file location, so tests work in checkout and CI without bundle resources:

```swift
import Foundation

enum Fixtures {
    static var dir: URL {
        // …/apps/rupu-macos/RupuKit/Tests/RupuAPITests/FixtureLoader.swift → …/apps/rupu-macos/Fixtures
        URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()  // RupuAPITests
            .deletingLastPathComponent()  // Tests
            .deletingLastPathComponent()  // RupuKit
            .appendingPathComponent("Fixtures")
    }
    static func data(_ name: String) throws -> Data {
        try Data(contentsOf: dir.appendingPathComponent(name))
    }
}
```

- [ ] **Step 2: Write failing decode tests:**

```swift
import Testing
import Foundation
@testable import RupuAPI

@Test func decodesHostInfoFixture() throws {
    let info = try JSONDecoder().decode(HostInfo.self, from: Fixtures.data("host_info.json"))
    #expect(info.version == "0.71.0")
    #expect(info.capabilities.permissionModes == ["ask", "bypass", "readonly"])
}

@Test func decodesEveryEventFixtureVariant() throws {
    let events = try JSONDecoder().decode([CPEvent].self, from: Fixtures.data("events.json"))
    #expect(events.count >= 18)
    #expect(!events.contains { if case .unknown = $0 { true } else { false } })
    if case let .stepCompleted(runID, stepID, success, durationMS, host) = events[4] {
        #expect(runID == "run-01" && stepID == "plan" && success && durationMS == 4200 && host == "mini")
    } else { Issue.record("events[4] should be step_completed") }
}

@Test func unknownEventTypeDecodesAsUnknownNotError() throws {
    let json = Data(#"{"type":"future_thing","run_id":"r9"}"#.utf8)
    let ev = try JSONDecoder().decode(CPEvent.self, from: json)
    #expect(ev == .unknown(type: "future_thing", runID: "r9"))
}
```

- [ ] **Step 3: Run** `make macos-test` — Expected: FAIL (types undefined).
- [ ] **Step 4: Implement `CPEvent.swift`** — mirror every Rust variant; decode on the `type` tag; unknown tag → `.unknown` (forward compatibility, never a throw):

```swift
public enum CPEvent: Equatable, Sendable {
    case runStarted(runID: String, workflowPath: String, startedAt: String)
    case stepStarted(runID: String, stepID: String, kind: String, agent: String?, host: String?)
    case stepWorking(runID: String, stepID: String, note: String?, transcriptPath: String?)
    case stepAwaitingApproval(runID: String, stepID: String, reason: String)
    case stepCompleted(runID: String, stepID: String, success: Bool, durationMS: UInt64, host: String?)
    case stepFailed(runID: String, stepID: String, error: String)
    case stepSkipped(runID: String, stepID: String, reason: String)
    case unitStarted(runID: String, stepID: String, index: Int, unitKey: String, agent: String?, transcriptPath: String, host: String?)
    case unitCompleted(runID: String, stepID: String, index: Int, unitKey: String, success: Bool, tokensIn: UInt64, tokensOut: UInt64, host: String?)
    case panelRound(runID: String, stepID: String, round: UInt32, maxIterations: UInt32, maxSeverityRemaining: String?)
    case runCompleted(runID: String, status: String, finishedAt: String)
    case runFailed(runID: String, error: String, finishedAt: String)
    case runPaused(runID: String)
    case runResumed(runID: String)
    case stepPaused(runID: String, stepID: String)
    case stepResumed(runID: String, stepID: String)
    case dispatchStarted(runID: String, subRunID: String, agent: String?, transcriptPath: String)
    case dispatchCompleted(runID: String, subRunID: String, success: Bool, tokensIn: UInt64, tokensOut: UInt64)
    case unknown(type: String, runID: String?)
}
```

Implement `Decodable` with a `CodingKeys` enum covering the snake_case field names (`run_id`, `step_id`, `duration_ms`, `workflow_path`, `started_at`, `finished_at`, `transcript_path`, `unit_key`, `tokens_in`, `tokens_out`, `max_iterations`, `max_severity_remaining`, `sub_run_id`, `event_version`) and a `switch typeTag` over the exact Rust tags (`run_started`, `step_started`, `step_working`, `step_awaiting_approval`, `step_completed`, `step_failed`, `step_skipped`, `unit_started`, `unit_completed`, `panel_round`, `run_completed`, `run_failed`, `run_paused`, `run_resumed`, `step_paused`, `step_resumed`, `dispatch_started`, `dispatch_completed`). Timestamps and `kind`/`status` decode as `String` in Phase 1 (typed enums arrive with the screens that render them — YAGNI). Add `public var runID: String?` switch.

- [ ] **Step 5: Implement `Models.swift`** (`HostInfo`, `HostCapabilities` with explicit snake_case `CodingKeys`, `CPEventRow` whose `init(from:)` decodes `ts`/`pos` then delegates the same decoder to `CPEvent`) **and `CPClient.swift`**:

```swift
public actor CPClient {
    let config: CPConfig
    let session: URLSession

    public init(config: CPConfig, session: URLSession = .shared) { ... }

    public func hostInfo() async throws -> HostInfo { try await get("api/host/info") }
    public func recentEvents(limit: Int = 200) async throws -> [CPEventRow] {
        try await get("api/events", query: [URLQueryItem(name: "limit", value: String(limit))])
    }

    func get<T: Decodable>(_ path: String, query: [URLQueryItem] = []) async throws -> T {
        // build URL from config.baseURL + path + query
        // set "Authorization: Bearer <token>" when config.token != nil
        // 401 → CPError.unauthorized; non-2xx → CPError.http(status:body:)
        // URLError → CPError.transport; DecodingError → CPError.decoding
    }
}
```

- [ ] **Step 6: `CPClientTests.swift`** — use a `URLProtocol` stub (`StubURLProtocol` registered in an ephemeral `URLSessionConfiguration`) to assert: (a) `hostInfo()` hits `/api/host/info` and decodes the fixture bytes; (b) the Bearer header is present exactly when a token is configured; (c) a 401 maps to `.unauthorized` and a 500 to `.http(status: 500, ...)`.
- [ ] **Step 7: Run** `make macos-test` — Expected: PASS.
- [ ] **Step 8: Commit** — `feat(macos): RupuAPI models, CPEvent decoding, CPClient with fixture-backed tests`

---

### Task 6: SSE — parser + EventStreamClient

**Files:**
- Create: `RupuKit/Sources/RupuAPI/SSE.swift`
- Test: `RupuKit/Tests/RupuAPITests/SSEParserTests.swift`

**Interfaces:**
- Consumes: `CPEvent`, `CPConfig`, `CPError` (Task 5).
- Produces:
  - `public struct SSEFrame: Equatable, Sendable { public var event: String?; public var data: String }`
  - `public struct SSELineParser: Sendable { public init(); public mutating func feed(line: String) -> SSEFrame? }` — pure, per-line: accumulates `data:` lines (multi-line joined with `\n`), captures `event:`, ignores `:` comments and `id:`/`retry:`, dispatches the frame on an empty line, resets after dispatch.
  - `public final class EventStreamClient: Sendable { public init(url: URL, token: String?, session: URLSession = .shared); public func events() -> AsyncStream<CPEvent> }` — connects with `Accept: text/event-stream` (+ Bearer), feeds `URLSession.bytes` lines through `SSELineParser`, decodes each frame's `data` as `CPEvent` (undecodable frames are skipped, not fatal), reconnects on stream end/error with backoff 1s→2s→4s→…→30s cap, resets backoff after a healthy frame. Cancellation of the consuming task tears the connection down.

- [ ] **Step 1: Write failing parser tests:**

```swift
import Testing
@testable import RupuAPI

@Test func parsesSingleDataFrame() {
    var p = SSELineParser()
    #expect(p.feed(line: #"data: {"type":"run_paused","run_id":"r1"}"#) == nil)
    let frame = p.feed(line: "")
    #expect(frame?.data == #"{"type":"run_paused","run_id":"r1"}"#)
}
@Test func joinsMultiLineData() {
    var p = SSELineParser()
    _ = p.feed(line: "data: {\"a\":")
    _ = p.feed(line: "data: 1}")
    #expect(p.feed(line: "")?.data == "{\"a\":\n1}")
}
@Test func ignoresCommentsAndKeepAlives() {
    var p = SSELineParser()
    #expect(p.feed(line: ": keep-alive") == nil)
    #expect(p.feed(line: "") == nil)   // blank with no pending data → no frame
}
@Test func capturesEventName() {
    var p = SSELineParser()
    _ = p.feed(line: "event: message")
    _ = p.feed(line: "data: x")
    #expect(p.feed(line: "") == SSEFrame(event: "message", data: "x"))
}
```

- [ ] **Step 2: Run** `make macos-test` — Expected: FAIL.
- [ ] **Step 3: Implement** `SSELineParser` (handle both `data:` and `data: ` prefixes per the SSE spec — strip one leading space), then `EventStreamClient` per the interface above. Keep the network loop small; all framing logic stays in the tested parser.
- [ ] **Step 4: Run** `make macos-test` — Expected: PASS.
- [ ] **Step 5: Live smoke (manual, evidence in PR)** — with a real CP running (`rupu cp serve`), run a scratch swift script or unit-test-with-env-guard that connects `EventStreamClient` to `http://127.0.0.1:<port>/api/events/stream` and prints the first frames while a workflow runs. Paste observed output into the PR description.
- [ ] **Step 6: Commit** — `feat(macos): SSE line parser + reconnecting EventStreamClient`

---

### Task 7: RupuBackend — discovery, version gate, Keychain, embedded server, health

**Files:**
- Create: `RupuKit/Sources/RupuBackend/BackendMode.swift`, `RupuDiscovery.swift`, `VersionGate.swift`, `KeychainTokenStore.swift`, `EmbeddedServer.swift`, `HealthMonitor.swift`
- Test: `RupuKit/Tests/RupuBackendTests/VersionGateTests.swift`, `HealthMonitorTests.swift`, `RupuDiscoveryTests.swift`

**Interfaces:**
- Consumes: `CPClient`, `HostInfo`, `CPConfig` (Task 5).
- Produces:
  - `public enum BackendMode: Codable, Equatable, Sendable { case embedded(port: Int), remote(url: URL) }` + `public var baseURL: URL` (embedded → `http://127.0.0.1:<port>/`)
  - `public enum BackendHealth: Equatable, Sendable { case starting, healthy(version: String), degraded(String), down(String), incompatible(serverVersion: String) }`
  - `public struct RupuDiscovery { public static func find(override: String?, searchPaths: [String] = defaultPaths, which: (String) -> String? = loginShellWhich) -> String? }` — order: override path (if executable) → `which rupu` via the user's login shell (`/bin/zsh -lc "which rupu"`) → `defaultPaths` = `/opt/homebrew/bin/rupu`, `/usr/local/bin/rupu`, `~/.local/bin/rupu` (first executable wins). Injectable `which` + `searchPaths` for tests.
  - `public enum VersionGate { public static let minimum = "0.71.0"; public static func compatible(_ version: String) -> Bool }` — numeric dot-segment compare, tolerant of a trailing prerelease suffix (`0.72.0-beta.1` compares as `0.72.0`).
  - `public struct KeychainTokenStore { public init(service: String = "com.section9labs.rupu"); public func save(token: String, account: String) throws; public func load(account: String) -> String?; public func delete(account: String) throws }` — generic-password items; `account` = the remote base URL string.
  - `public actor EmbeddedServer { public enum Origin: Equatable { case attached, spawned(pid: Int32) }; public init(binaryPath: String, port: Int, probe: @Sendable (URL) async -> Bool); public func start() async throws -> Origin; public func stop(keepRunning: Bool) }` — `start()`: probe `http://127.0.0.1:<port>/api/host/info`; success → `.attached`; else spawn `Process` (`binaryPath`, args `["cp", "serve", "--port", String(port)]`, own process group), then poll the probe (500ms interval, 20s timeout) → `.spawned(pid)` or throw. `stop(keepRunning:)` terminates the process group only when origin is `.spawned` and `keepRunning == false`; **never** touches an attached server.
  - `@MainActor @Observable public final class HealthMonitor { public private(set) var health: BackendHealth; public init(probe: @escaping @Sendable () async throws -> HostInfo, interval: Duration = .seconds(5)); public func start(); public func stop(); public func pollOnce() async }` — `pollOnce`: probe success + `VersionGate.compatible` → `.healthy(version)`; success + incompatible → `.incompatible`; first failure after healthy → `.degraded(msg)`; failure while starting/degraded → `.down(msg)`.

- [ ] **Step 1: Write failing tests:**

```swift
import Testing
import Foundation
@testable import RupuBackend
import RupuAPI

@Test func versionGate() {
    #expect(VersionGate.compatible("0.71.0"))
    #expect(VersionGate.compatible("0.72.3"))
    #expect(VersionGate.compatible("1.0.0"))
    #expect(!VersionGate.compatible("0.70.9"))
    #expect(VersionGate.compatible("0.72.0-beta.1"))
    #expect(!VersionGate.compatible("garbage"))
}

@Test func discoveryPrefersOverrideThenWhichThenPaths() {
    let tmp = FileManager.default.temporaryDirectory.appendingPathComponent("fake-rupu-\(UUID())").path
    FileManager.default.createFile(atPath: tmp, contents: Data(), attributes: [.posixPermissions: 0o755])
    #expect(RupuDiscovery.find(override: tmp, which: { _ in nil }) == tmp)
    #expect(RupuDiscovery.find(override: nil, searchPaths: [], which: { _ in tmp }) == tmp)
    #expect(RupuDiscovery.find(override: nil, searchPaths: [tmp], which: { _ in nil }) == tmp)
    #expect(RupuDiscovery.find(override: nil, searchPaths: [], which: { _ in nil }) == nil)
}

// Test helper, defined in this test file:
// final class LockedBox: @unchecked Sendable {
//     private let lock = NSLock(); private var v: Bool
//     init(_ v: Bool) { self.v = v }
//     var value: Bool {
//         get { lock.withLock { v } }
//         set { lock.withLock { v = newValue } }
//     }
// }
@MainActor @Test func healthMonitorTransitions() async {
    let flag = LockedBox(false)
    let monitor = HealthMonitor(probe: {
        if flag.value { return HostInfo(version: "0.71.0", capabilities: .init(backends: [], scmHosts: [], permissionModes: [])) }
        throw CPError.transport("refused")
    })
    #expect(monitor.health == .starting)
    await monitor.pollOnce()
    #expect(monitor.health == .down("refused"))
    flag.value = true
    await monitor.pollOnce()
    #expect(monitor.health == .healthy(version: "0.71.0"))
    flag.value = false
    await monitor.pollOnce()
    if case .degraded = monitor.health {} else { Issue.record("healthy → failure should be degraded, got \(monitor.health)") }
}
```

(`HostInfo` needs a public memberwise init for this test — add one to Task 5's model. `CPError.transport` message extraction: `.down`/`.degraded` carry `String(describing: error)`; assert with prefix matching if exact form differs.)

- [ ] **Step 2: Run** `make macos-test` — Expected: FAIL.
- [ ] **Step 3: Implement** all six files per the interfaces. `EmbeddedServer` gets no unit test (spawning is integration; verified in Task 9's smoke) but its probe/poll loop must be factored so `start()` logic is readable in one screen. `KeychainTokenStore` gets no CI test (Keychain in CI is flaky); verified manually in Task 9.
- [ ] **Step 4: Run** `make macos-test` — Expected: PASS.
- [ ] **Step 5: Commit** — `feat(macos): RupuBackend — discovery, version gate, keychain, embedded attach-or-spawn, health monitor`

---

### Task 8: RupuStore + shell — window, sidebar, toolbar, routing

**Files:**
- Create: `RupuKit/Sources/RupuStore/AppModel.swift`, `Route.swift`
- Create: `RupuKit/Sources/RupuShell/RootView.swift`, `Sidebar.swift`, `ShellToolbar.swift`, `PlaceholderScreen.swift`, `SettingsView.swift`
- Modify: `apps/rupu-macos/App/RupuApp.swift`
- Test: `RupuKit/Tests/RupuStoreTests/RoutingTests.swift`

**Interfaces:**
- Consumes: `BackendHealth`, `HealthMonitor` (Task 7), RupuDesign tokens (Task 3).
- Produces:
  - `public enum RunKindFilter: String, CaseIterable, Sendable { case all, agents, workflows, autoflows, sessions }`
  - `public enum Route: Hashable, Sendable { case overview, activity(RunKindFilter), projects, security, library, fleet, usage }`
  - `public enum TimeRange: String, CaseIterable, Sendable { case d7 = "7d", d30 = "30d", all }`
  - `@MainActor @Observable public final class AppModel { public var route: Route; public var range: TimeRange; public var backendHealth: BackendHealth; public var liveConnected: Bool; public var liveEventCount: Int; public var onboardingComplete: Bool (UserDefaults-backed); public init(defaults: UserDefaults = .standard) }`
  - `public struct RootView: View { public init(model: AppModel) }`

- [ ] **Step 1: Failing routing test** — sidebar selection and the Activity kind filter are ONE state (HANDOFF §Window model):

```swift
import Testing
@testable import RupuStore

@MainActor @Test func sidebarLeavesAndKindFilterAreSameState() {
    let model = AppModel(defaults: .init(suiteName: "test-\(UUID())")!)
    #expect(model.route == .overview)
    model.route = .activity(.workflows)
    #expect(model.selectedSidebarItem == SidebarItem.runsLeaf(.workflows))
    model.route = .activity(.all)
    #expect(model.selectedSidebarItem == SidebarItem.runs)
}
@MainActor @Test func onboardingFlagPersists() {
    let suite = "test-\(UUID())"
    let d = UserDefaults(suiteName: suite)!
    AppModel(defaults: d).onboardingComplete = true
    #expect(AppModel(defaults: d).onboardingComplete)
}
```

(`SidebarItem` is a `public enum SidebarItem: Hashable { case overview, runs, runsLeaf(RunKindFilter), projects, security, library, fleet, usage }` derived from/driving `route` via a computed property with getter+setter.)

- [ ] **Step 2: Run** `make macos-test` — Expected: FAIL. Implement `Route.swift` + `AppModel.swift`; re-run → PASS.
- [ ] **Step 3: Implement the shell views:**
  - `RootView`: `NavigationSplitView`; detail switches on `model.route` to `PlaceholderScreen(title:)` for every screen (Phase 2+ replaces them). 1440×900 default handled in the App scene (`.defaultSize(width: 1440, height: 900)`, `.frame(minWidth: 1150, minHeight: 760)`).
  - `Sidebar`: 216pt fixed width; `NSVisualEffectView` `.sidebar` material via an `NSViewRepresentable` background; sections **Overview · Runs (leaves: Agent runs / Workflows / Autoflows / Sessions) · Subjects (Projects) · Security · Library · Fleet · Usage**; `List(selection: $model.selectedSidebarItem)`. Footer: host status row — dot colored by `backendHealth` (`healthy`→`Color.status(.done)`, `degraded`→`.waiting`, `down`/`incompatible`→`.fail`, `starting`→`.pause`) + `MicroLabel` text. **No Settings row** (Settings scene owns ⌘,).
  - `ShellToolbar`: screen title (from route) · range segmented picker (7d/30d/all, bound to `model.range`) · live pill (`liveConnected` → brand-tinted "LIVE" MicroLabel with beacon dot; else dim "OFFLINE") · appearance toggle (System/Light/Dark via `@AppStorage("appearance")` applied with `.preferredColorScheme`). Project-scope picker, ⌘K search, and Edit are **not rendered** in Phase 1 (no wired backend → no control, per global constraint).
  - `PlaceholderScreen`: panel-chrome page (Task 3 `PanelStyle`) with the screen title and a MicroLabel line "NOT BUILT YET — PHASE <n>" (n per spec §8 table). No fake stats, no dead controls.
  - `SettingsView`: General tab only — appearance picker, embedded port field, rupu binary override path field (bound to UserDefaults keys the backend wiring in Task 9 reads); Connection/Providers/Notifications/Dashboard tabs arrive in Phase 6.
- [ ] **Step 4: Wire the app target** — `RupuApp` builds `AppModel` + `RootView`, adds `Settings { SettingsView() }` scene.
- [ ] **Step 5: Verify** — `make macos-test` (PASS) + `make macos-run`: window opens with sidebar/toolbar chrome in both light and dark appearance (toggle via the toolbar control). Screenshot both appearances (computer-use) for the PR.
- [ ] **Step 6: Commit** — `feat(macos): app shell — sidebar, toolbar, routing, placeholder screens, settings stub`

---

### Task 9: Onboarding + backend wiring + live pill

**Files:**
- Create: `RupuKit/Sources/RupuShell/OnboardingView.swift`, `RupuKit/Sources/RupuStore/BackendController.swift`
- Modify: `RupuKit/Sources/RupuShell/RootView.swift`, `apps/rupu-macos/App/RupuApp.swift`
- Test: `RupuKit/Tests/RupuStoreTests/BackendControllerTests.swift`

**Interfaces:**
- Consumes: everything from Tasks 5–8.
- Produces: `@MainActor @Observable public final class BackendController { public private(set) var mode: BackendMode?; public var health: BackendHealth (forwarded from HealthMonitor); public private(set) var origin: EmbeddedServer.Origin?; public func configureEmbedded(port: Int) async; public func configureRemote(url: URL, token: String) async; public func client() -> CPClient?; public func eventStream() -> EventStreamClient?; public func shutdown(keepRunning: Bool) async }` — persists mode in UserDefaults (`backend.mode` JSON) and the remote token in Keychain; on app launch with a persisted mode it reconnects without onboarding.

- [ ] **Step 1: Failing tests** — mode persistence round-trip (`BackendMode` Codable JSON in an isolated `UserDefaults` suite: configure embedded(7420) → new controller instance restores `.embedded(port: 7420)`); remote configure stores mode without touching real Keychain (inject a `TokenStoring` protocol — `KeychainTokenStore` conforms; tests use an in-memory fake).
- [ ] **Step 2: Run** `make macos-test` — FAIL → implement `BackendController` → PASS. Embedded path: `RupuDiscovery.find` (reading the Settings override key) → missing binary sets `health = .down("rupu not found — install rupu or set the path in Settings")`; found → `EmbeddedServer.start()` → start `HealthMonitor` with `CPClient.hostInfo`.
- [ ] **Step 3: OnboardingView** (artboard 03) — shown as a sheet when `!model.onboardingComplete`: two cards (Embedded / Remote). Embedded card: discovery status line (found path or install hint), port field (default 7420), "Start" button → `configureEmbedded`. Remote card: URL field, token SecureField, "Connect" → `configureRemote` (token → Keychain, never UserDefaults). Success (first `.healthy`) sets `onboardingComplete = true` and dismisses; `.incompatible` shows the blocking version banner with the `rupu update` hint (HANDOFF/spec §5).
- [ ] **Step 4: Live pill wiring** — when health becomes `.healthy`, `RootView` starts one task consuming `backend.eventStream()!.events()`: sets `model.liveConnected = true` on first frame/successful connect, increments `model.liveEventCount` per event, `liveConnected = false` on disconnect (stream client keeps reconnecting). This is the Phase 1 end-to-end proof that REST + SSE + auth all work; Phase 2 replaces the consumer with real stores.
- [ ] **Step 5: App lifecycle** — `RupuApp` owns `BackendController`; on termination (`NSApplication.willTerminateNotification`) call `shutdown(keepRunning: UserDefaults "keepServerRunning", default false)` — kills only spawned servers, never attached ones. Add the "Keep server running when app quits" toggle to SettingsView General.
- [ ] **Step 6: Integration smoke (manual, evidence in PR)** — three scenarios against the real installed rupu: (a) no CP on port → app spawns one (`ps` shows child; quit kills it); (b) pre-running `rupu cp serve --port 7420` → app attaches (quit leaves it alive); (c) remote mode against the same server via URL+token → healthy footer + LIVE pill; then `rupu run` a workflow and watch `liveEventCount` climb. Record all three outcomes in the PR description.
- [ ] **Step 7: Run** `make macos-test && make macos-build` — PASS. Commit — `feat(macos): onboarding, backend controller, embedded attach-or-spawn wiring, live SSE pill`

---

### Task 10: CI macOS lane + CLAUDE.md + final validation

**Files:**
- Modify: `.github/workflows/ci.yml` (new job)
- Modify: `CLAUDE.md` (new rupu.app section)

**Interfaces:**
- Consumes: Make targets (Task 2), the full app (Tasks 3–9).
- Produces: CI job `macos-app`; updated project docs.

- [ ] **Step 1: Add the CI job** (append to ci.yml's jobs; no cargo needed):

```yaml
  macos-app:
    name: macos-app (swift test + xcodebuild)
    runs-on: macos-15
    steps:
      - uses: actions/checkout@v4
      - name: Install XcodeGen
        run: brew list xcodegen >/dev/null 2>&1 || brew install xcodegen
      - name: RupuKit tests
        run: make macos-test
      - name: App target builds
        run: make macos-build
```

Note: the Swift fixture-decoding tests read `apps/rupu-macos/Fixtures/` from the checkout, and the Rust drift tests run in the existing linux lane (`cargo test --workspace` includes `rupu-cp`) — both sides of the contract are in CI.

- [ ] **Step 2: CLAUDE.md** — add a `## rupu.app (SwiftUI macOS)` section: location `apps/rupu-macos/`; thin-app-target rule; XcodeGen (`make macos-gen`, `.xcodeproj` gitignored — never edit it); module map (RupuAPI/RupuBackend/RupuStore/RupuDesign/RupuShell + one module per screen later); no third-party Swift deps; fixture rig (`make macos-fixtures` after changing any serde type the app consumes); programmatic design tokens (HANDOFF.md is the visual contract); GUI validation rule (matt runs the app before UI PR merges); default port 7420; `VersionGate.minimum` must be bumped consciously, not ambiently. Add the umbrella spec to the "Read first" list.
- [ ] **Step 3: Full local gate** — Run: `make macos-test && make macos-build && make lint && make test`. Expected: all green.
- [ ] **Step 4: Visual validation** — computer-use screenshots: onboarding sheet, main window light + dark, footer health states (healthy + down by stopping the server), LIVE pill during a workflow run. Attach to PR.
- [ ] **Step 5: Commit, push (explicit refspec), open the foundation PR** — body lists the Task 6/9 smoke evidence and screenshots. **matt runs the app before merge.**

---

## Self-review notes

- Spec coverage: §2 amendments (Task 7 discovery + attach-or-spawn), §3 layout/targets (Task 2), §4 modules (Tasks 3–8), §5 backend manager incl. version gate + keep-running (Tasks 7/9), §6 REST+SSE+re-snapshot discipline (Tasks 5/6; store-level re-snapshot lands with Phase 2's real stores), §7 fixtures (Tasks 4/5), §8 Phase-1 row (`host_info`, `events` covered; `config` read is *not* consumed by any Phase-1 surface — deferred to its first real consumer, Settings/Phase 6; the spec §8 parity ledger row moves accordingly), §9 error/null/no-noop rules (global constraints + Tasks 3/8), §10 deletion (Task 1).
- Deliberate deferrals: ⌘K search field, project-scope picker, Edit button (no backend to wire — HANDOFF places them in later phases' screens); typed `RunStatus`/`StepKind` Swift enums (arrive with Phase 2 rendering).
