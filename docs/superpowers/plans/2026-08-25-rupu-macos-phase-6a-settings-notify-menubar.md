# rupu.app macOS Phase 6A — Settings, Notifications, Menu Bar Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The ambient control layer — a real four-tab Settings scene (including server config read/write), local notifications off the firehose, and a menu bar extra with needs-you triage.

**Architecture:** New `RupuAPI` config surface (`ConfigModels` + 4 CPClient methods) → `ConfigStore` in RupuStore → a restructured `SettingsView` (General / Connection / Config / Notifications). A `RunNotifier` consumes its own firehose stream and posts `UNUserNotificationCenter` notifications per `@AppStorage` prefs, deep-linking into run detail. A `MenuBarExtra` scene backed by `MenuBarStore` (dashboard poll + `deriveNeedsYou` reuse) with inline gate actions through the shared `PendingActions`.

**Tech Stack:** SwiftUI (`MenuBarExtra`, `Settings` scene), `UserNotifications`, Swift Testing, Rust fixture emission from `rupu-cp` serde types.

**Spec:** `docs/superpowers/specs/2026-08-25-rupu-macos-phase-6-ambient-design.md` (§1–§3, §7–§9)

## Global Constraints

- Any Swift test touching a SwiftUI `View`-type member MUST be `@Test @MainActor` (CI isolates strictly; local compiles fine either way).
- Test-stub statics (URLProtocol stubs etc.) use generation-token isolation — never a bare shared static (the `DashboardStubURLProtocol` precedent).
- EVERY task ends with the FULL `make macos-test` suite green (and `cargo test -p rupu-cp` for tasks touching Rust), regardless of what the task touched. A red test is never "owned by a later task".
- Timing-sensitive tests condition-poll (`pollUntil` idiom) with WIDE margins; never assert on sleeps.
- Rust formatting: per-file `rustfmt` only — NEVER package-wide `cargo fmt` (main is fmt-dirty under the pinned toolchain).
- No third-party Swift dependencies. Workspace deps only on the Rust side.
- Comments/docs cite sources truthfully — verify any "matches web/server X" claim against the actual source before writing it.
- Stores follow the established idioms: `@MainActor @Observable`, generation-guarded refetch, `activate/deactivate` task loops with `weak self` per iteration, client-identity rebuild at screens.
- Honest UI: nulls render `—`; disabled controls carry a reason; no dead controls, no stub tabs.
- Implementers never dispatch subagents. Never use bare `git stash` / `git stash pop`.

---

### Task 1: Config fixture + fixture test (Rust)

**Files:**
- Create: `apps/rupu-macos/Fixtures/config_view.json` (generated)
- Modify: `crates/rupu-cp/tests/macos_fixtures.rs`
- Modify (visibility only, if needed): `crates/rupu-cp/src/api/config.rs`

**Interfaces:**
- Produces: `config_view.json` — a serialized `ConfigView` (the exact `GET /api/config` shape: `effective`, `provenance` map of dotted key → `{source, locked}`, `raw_global`, `raw_project`, `cp`, `status: {bind, token_set, restart_required_keys}`).

**Steps:**

- [ ] **Step 1:** Read `crates/rupu-cp/src/api/config.rs` (the `ConfigView`/`RuntimeStatus` types and `get_config`) and the existing fixture-test pattern in `crates/rupu-cp/tests/macos_fixtures.rs` (24 tests; each constructs the REAL serde type and writes/compares `apps/rupu-macos/Fixtures/<name>.json`).
- [ ] **Step 2:** `ConfigView` and `RuntimeStatus` are private to `api/config.rs`. Hoist them to `pub(crate)` (fields included) so the fixture test can construct them — the established precedent from earlier fixture tasks. Do NOT change the route handlers.
- [ ] **Step 3:** Add fixture test `config_view_fixture` constructing a representative `ConfigView`:
  - `effective`: a small `serde_json::json!` tree with at least `default_model`, a `[providers]` sub-table containing a **dotted-name provider key** (`"GLM-5.2-FP8"` — the canonical quoted-encoding stress case), and `[cp]` keys.
  - `provenance`: at least four entries covering the full matrix — `KeySource::Global` unlocked, `KeySource::Project` unlocked, `KeySource::Default` unlocked, and one `locked: true` entry; include one key whose middle segment is quoted (`providers."GLM-5.2-FP8".model`) exactly as `rupu_config::resolve::dotted()` emits it.
  - `raw_global`: a short real TOML string (with a `[policy]` `lock = [...]` line); `raw_project: Some(...)` with distinct content.
  - `status`: `bind: "127.0.0.1:7420"`, `token_set: false`, `restart_required_keys: ["bind", "token"]`.
- [ ] **Step 4:** Run `cargo test -p rupu-cp --test macos_fixtures` in regenerate mode (the rig's env-var switch — read how the existing 24 emit) to write `config_view.json`, then run the full `cargo test -p rupu-cp` green.
- [ ] **Step 5:** `make macos-test` (full suite — must stay green; nothing consumes the fixture yet).
- [ ] **Step 6:** Commit: `test(macos): config_view fixture from rupu-cp serde types`

---

### Task 2: RupuAPI config surface

**Files:**
- Create: `apps/rupu-macos/RupuKit/Sources/RupuAPI/ConfigModels.swift`
- Modify: `apps/rupu-macos/RupuKit/Sources/RupuAPI/CPClient.swift`
- Test: `apps/rupu-macos/RupuKit/Tests/RupuAPITests/ConfigModelsTests.swift`

**Interfaces:**
- Consumes: `config_view.json` (Task 1); CPClient's existing `getJSON`/request helpers and error type (read `CPClient.swift` first — reuse its exact request/decode idiom and its HTTP-status error surface).
- Produces:
  ```swift
  public enum APIKeySource: String, Decodable, Sendable { case global, project, `default` }
  public struct APIKeyProvenance: Decodable, Equatable, Sendable { public let source: APIKeySource; public let locked: Bool }
  public struct APIRuntimeStatus: Decodable, Equatable, Sendable {
      public let bind: String; public let tokenSet: Bool; public let restartRequiredKeys: [String]
      // CodingKeys: token_set, restart_required_keys
  }
  public struct APIConfigView: Decodable, Sendable {
      public let effective: JSONValue        // reuse the existing untyped-JSON representation if one exists in RupuAPI; otherwise a minimal JSONValue enum (string/number/bool/null/array/object) added here
      public let provenance: [String: APIKeyProvenance]
      public let rawGlobal: String           // raw_global
      public let rawProject: String?         // raw_project
      public let status: APIRuntimeStatus
      // NOTE: the wire's `cp` field is deliberately not decoded (redundant with effective.cp) — doc-comment this.
  }
  // CPClient:
  public func fetchConfig(project: String? = nil) async throws -> APIConfigView   // GET /api/config[?project=]
  public func putConfigGlobal(raw: String) async throws                            // PUT /api/config/global {"raw": ...}
  public func putConfigProject(id: String, raw: String) async throws              // PUT /api/config/project/:id {"raw": ...}
  public func putConfigPolicy(lock: [String]) async throws                        // PUT /api/config/policy {"lock": ...}
  ```
- Search RupuAPI for an existing untyped-JSON value type before adding `JSONValue` — if `AnyCodable`-style machinery already exists (check how `effective`-like blobs are handled elsewhere, e.g. launcher inputs), reuse it.
- Write bodies send `{"raw": ...}` only — the `patch` form is deliberately unused (typed forms deferred; doc-comment it).

**Steps:**

- [ ] **Step 1:** Write failing decode tests: load `config_view.json` from the fixtures bundle (existing test-bundle idiom — copy from `CoverageModelsTests` or similar), decode `APIConfigView`, assert: the quoted-key provenance entry survives verbatim as a dictionary key; the locked entry's `locked == true`; `status.restartRequiredKeys == ["bind", "token"]`; `rawProject` non-nil.
- [ ] **Step 2:** Implement `ConfigModels.swift` + the four CPClient methods (PUTs follow the existing mutation-method idiom — find `approveRun`/`removeHost` for the request-building + error-mapping pattern; the three PUTs must surface HTTP 501 distinguishably through CPClient's existing error type so Task 4 can map it to read-only).
- [ ] **Step 3:** `make macos-test` full suite green.
- [ ] **Step 4:** Commit: `feat(macos): RupuAPI config surface (view decode + raw/policy writes)`

---

### Task 3: Dotted-key pure functions

**Files:**
- Create: `apps/rupu-macos/RupuKit/Sources/RupuStore/DottedKey.swift`
- Test: `apps/rupu-macos/RupuKit/Tests/RupuStoreTests/DottedKeyTests.swift`

**Interfaces:**
- Produces:
  ```swift
  public enum DottedKey {
      /// Lenient quote-aware split — the READ-side decoder. Splits on `.` except inside a
      /// `"…"`-quoted segment (`\"` unescapes to `"`). Malformed quoting falls back to a
      /// naive split — display-only consumer, degrade never crash.
      public static func split(_ key: String) -> [String]
      /// Canonical encode of one segment: wrap in quotes (escaping `"` as `\"`) iff the
      /// segment contains `.` or `"`; bare otherwise.
      public static func quoteSegment(_ s: String) -> String
      /// join = segments.map(quoteSegment).joined(separator: ".")
      public static func join(_ segments: [String]) -> String
  }
  ```
- This is the Swift port of the canonical contract shared by `rupu_config::resolve::dotted()`, `config_write::split_dotted_key`, `api/config.rs`, and web `ConfigEditor.tsx::quoteSegment`/split. **Strict-write / lenient-read asymmetry is deliberate** — mirror the web's doc comment honestly (verify wording against `crates/rupu-cp/web/src/components/ConfigEditor.tsx:46-70` before citing).

**Steps:**

- [ ] **Step 1:** Read `crates/rupu-cp/web/src/components/ConfigEditor.tsx` (quoteSegment + the split inverse) and `crates/rupu-cp/web/src/components/ConfigEditor.dotted.test.tsx` — the web test table is the parity source.
- [ ] **Step 2:** Write the failing test table mirroring the web's cases plus round-trips: `providers."GLM-5.2-FP8".model` ↔ `["providers", "GLM-5.2-FP8", "model"]`; a segment containing `"`; a plain `a.b.c`; malformed unterminated quote (lenient fallback); empty segment tolerance on read.
- [ ] **Step 3:** Implement; all table cases green.
- [ ] **Step 4:** `make macos-test` full suite green.
- [ ] **Step 5:** Commit: `feat(macos): DottedKey — canonical quote-aware split/encode (web parity)`

---

### Task 4: ConfigStore

**Files:**
- Create: `apps/rupu-macos/RupuKit/Sources/RupuStore/ConfigStore.swift`
- Test: `apps/rupu-macos/RupuKit/Tests/RupuStoreTests/ConfigStoreTests.swift`

**Interfaces:**
- Consumes: `CPClient.fetchConfig/putConfig*` (Task 2); the established `BlockState` machinery; the URLProtocol stub rig with generation tokens.
- Produces:
  ```swift
  @MainActor @Observable public final class ConfigStore {
      public private(set) var view: BlockState<APIConfigView>   // (or the repo's equivalent block primitive — match ProjectDetailStore)
      public private(set) var selectedProject: String?           // nil = global-only view
      public private(set) var readOnly: Bool                     // true once any write (or a probe) returns 501
      public private(set) var saveError: String?                 // last 400 body, cleared on next attempt
      public private(set) var saving: Bool
      public private(set) var lastSaveRestartKeys: [String]      // restart_required from a global save response ∩ status.restartRequiredKeys — see Step 2 ruling
      public func load(client: CPClient, project: String?) async // generation-guarded
      public func saveGlobalRaw(_ raw: String, client: CPClient) async -> Bool
      public func saveProjectRaw(_ raw: String, client: CPClient) async -> Bool   // requires selectedProject
      public func savePolicy(lock: [String], client: CPClient) async -> Bool
  }
  ```
- Every successful save re-`load`s (the server is the source of truth — never patch `view` locally).
- 501 from any write sets `readOnly = true` and a clear saveError ("editing config requires `rupu cp serve`" — the server's own message). 400 bodies surface verbatim in `saveError` (they carry the TOML validation error).

**Steps:**

- [ ] **Step 1:** Read `ProjectDetailStore.swift` (or `CoverageDetailStore.swift`) for the exact block/generation idiom, and the stub-URLProtocol test rig used by its tests.
- [ ] **Step 2 (ruling to carry):** the global-save response's `restart_required` array is currently always `[]` on the wire (see `put_global` in `api/config.rs`); the *keys that would need a restart* live in `status.restart_required_keys`. The honest banner contract: after a successful global save, compare the saved raw against the pre-save raw for lines mentioning any `status.restartRequiredKeys` key is over-engineering — instead set `lastSaveRestartKeys = view.status.restartRequiredKeys` ONLY if the saved raw text differs from the previous `rawGlobal` on a line containing one of those key names (simple substring check, doc-commented as a heuristic banner, not a guarantee). Keep it honest: the banner text says "may require restart".
- [ ] **Step 3:** Failing tests: load happy path from fixture; generation guard (switch project mid-flight — stale response dropped); 501 write → `readOnly` + error; 400 write → `saveError` carries body, `readOnly` stays false; successful save triggers re-load (assert a second GET hit the stub).
- [ ] **Step 4:** Implement; tests green.
- [ ] **Step 5:** `make macos-test` full suite green.
- [ ] **Step 6:** Commit: `feat(macos): ConfigStore — load/save with 501 read-only + 400 surfacing`

---

### Task 5: Settings restructure — four tabs + Connection split

**Files:**
- Modify: `apps/rupu-macos/RupuKit/Sources/RupuShell/SettingsView.swift`
- Modify: `apps/rupu-macos/App/RupuApp.swift` (pass `model`/`backend` into `SettingsView`)
- Test: `apps/rupu-macos/RupuKit/Tests/RupuShellTests/SettingsViewTests.swift`

**Interfaces:**
- Consumes: existing `@AppStorage` keys (`appearance`, `embedded.port`, `rupu.binaryPath`, `keepServerRunning`); `AppModel`/`BackendController` (for the Config tab in Task 6 and the connection info display).
- Produces: `SettingsView(model: AppModel, backend: BackendController)` with a `TabView` of **General** (appearance only), **Connection** (embedded port, binary override, keep-running — moved verbatim from General; plus a read-only current-connection line: mode/port from backend state), **Config** (placeholder `ConfigTab(model:backend:)` stub view this task — replaced in Task 6; the stub must NOT ship a dead tab: it renders the store-less loading shell only if Task 6 is in the same PR, which it is), **Notifications** (stub for Task 7, same rationale).
- Width grows to 560 to fit the Config editors; per-tab `.frame` if the General tab looks sparse.

**Steps:**

- [ ] **Step 1:** Failing tests (`@Test @MainActor`): construct `SettingsView` with a fresh `AppModel`/`BackendController`; assert tab identity by rendering the four tab views' member accessors (follow the existing view-member test idiom in RupuShellTests).
- [ ] **Step 2:** Restructure `SettingsView` (init takes `model`/`backend`; four tabs; Connection content moved from General; update the doc comment — the "arrive in Phase 6" sentence comes true, rewrite it). Update `RupuApp.swift`'s `Settings` scene to pass the instances.
- [ ] **Step 3:** `make macos-build` + `make macos-test` full suite green.
- [ ] **Step 4:** Commit: `feat(macos): Settings — four tabs, Connection split out of General`

---

### Task 6: Config tab UI

**Files:**
- Create: `apps/rupu-macos/RupuKit/Sources/RupuShell/ConfigTab.swift` (container + sub-views: `EffectiveConfigList`, `RawConfigEditor`, `PolicyLockEditor`)
- Modify: `apps/rupu-macos/RupuKit/Sources/RupuShell/SettingsView.swift` (replace stub)
- Test: `apps/rupu-macos/RupuKit/Tests/RupuShellTests/ConfigTabTests.swift`

**Interfaces:**
- Consumes: `ConfigStore` (Task 4), `DottedKey.split` (Task 3), `CPClient.projects()` (project picker), RupuDesign tokens/chrome (StatusPill/Badge/Chip idioms — match existing screens).
- Produces, inside the tab (segmented: **Effective / Raw / Policy**):
  - **Effective**: searchable list grouped by top-level section (`DottedKey.split(key).first`), each row: remainder key (joined for display), value (rendered from the effective tree; scalars inline, arrays joined, tables recursed by the grouping), provenance chip (`global`/`project`/`default` — color-coded like the web), lock glyph when `locked`. Values resolve from `effective` by walking `DottedKey.split(key)`; a key that fails to resolve renders `—` (lenient-read contract).
  - **Raw**: layer picker (Global / project via `projects()` list); monospace `TextEditor` seeded from `rawGlobal`/`rawProject`; dirty tracking; Save button (disabled + reason when `readOnly` or not dirty); `saveError` inline in error styling; "may require restart" banner off `lastSaveRestartKeys`; discard-changes button when dirty. Switching layer with unsaved edits prompts (confirmationDialog) — never silently drops.
  - **Policy**: the current lock list (rows removable), an add field (free text, sent verbatim — server validates), a "lock a key…" picker populated from `provenance` keys (canonical encodings straight off the wire), Save via `savePolicy`. Disabled + reason under `readOnly`.
- All three share one `ConfigStore` owned by the tab (`@State`), activated on appear with the established client-identity rebuild (`backend.clientIdentity()`).

**Steps:**

- [ ] **Step 1:** Failing tests (`@Test @MainActor`): effective grouping — feed a store loaded from the fixture (stub rig), assert the quoted-key provenance row groups under `providers` (NOT split at the quoted dot) and its display remainder contains `GLM-5.2-FP8`; policy editor renders the fixture's lock entries; readOnly disables Save with a reason string.
- [ ] **Step 2:** Implement the three sub-views + container; wire into SettingsView replacing the stub.
- [ ] **Step 3:** `make macos-test` full suite + `make macos-build` green.
- [ ] **Step 4:** Commit: `feat(macos): Config tab — effective/provenance view, raw TOML editors, policy locks`

---

### Task 7: Notifications — prefs + RunNotifier

**Files:**
- Create: `apps/rupu-macos/RupuKit/Sources/RupuStore/RunNotifier.swift`
- Create: `apps/rupu-macos/RupuKit/Sources/RupuShell/NotificationsTab.swift`
- Modify: `apps/rupu-macos/RupuKit/Sources/RupuShell/SettingsView.swift` (replace stub)
- Modify: `apps/rupu-macos/App/RupuApp.swift` (own the notifier; UNUserNotificationCenter delegate for tap routing)
- Test: `apps/rupu-macos/RupuKit/Tests/RupuStoreTests/RunNotifierTests.swift`

**Interfaces:**
- Consumes: `BackendController.makeFirehoseStream(onConnectionChange:)` (independent-stream seam — same as OverviewScreen:399); `CPEvent` cases `stepAwaitingApproval` / `stepFailed` / `runFailed` / `runCompleted`; `AppModel.navigate(to: .runDetail(id:host:))`.
- Produces:
  ```swift
  @MainActor @Observable public final class RunNotifier {
      // @AppStorage-backed prefs (keys: "notify.gates", "notify.failures", "notify.completions" — all default false)
      public var notifyGates: Bool { get set }        // backed by UserDefaults; setter triggers ensureAuthorization on first enable
      public var notifyFailures: Bool { get set }
      public var notifyCompletions: Bool { get set }
      public private(set) var authorizationDenied: Bool
      public func activate(streamFactory: @escaping () -> EventStreamClient?)   // idempotent task loop, weak-self per iteration, reconnect with backoff on stream end
      public func deactivate()
      // Pure seam (unit-test THIS, not the loop):
      public func decision(for event: CPEvent, now: Date) -> NotificationContent?   // nil = suppressed (pref off, or dedup window hit)
  }
  ```
  `NotificationContent`: title/body/runID. Dedup: a `(runID, kindClass)` seen within the last 30s returns nil (replay-on-reconnect guard); the seen-map prunes on insert.
- Posting goes through a thin `NotificationPosting` protocol (prod: UNUserNotificationCenter wrapper; tests: recorder) — **never** call UNUserNotificationCenter in unit tests (no bundle context in SwiftPM tests; it throws).
- Authorization: requested only from a pref's first enable (`UNAuthorizationOptions: [.alert, .sound]`); denial sets `authorizationDenied` which the tab surfaces with a System Settings deep-link (`x-apple.systempreferences:com.apple.preference.notifications`).
- Tap routing: `AppDelegate` adopts `UNUserNotificationCenterDelegate`; `didReceive` reads `userInfo["runID"]` and calls `model.navigate(to: .runDetail(id: id, host: nil))` (notifications only ever describe local-CP runs — the firehose is the local firehose; doc-comment it) + brings the window forward.
- `RupuApp` owns one `RunNotifier`, activates it when the backend reports healthy (same seam the screens use), passes it to `NotificationsTab`.

**Steps:**

- [ ] **Step 1:** Failing tests on the pure seam: pref-off → nil; gate event with pref on → content with runID; same (runID, kind) twice within 30s → second nil; after 31s (inject `now`) → posts again; `runCompleted` maps to completions pref; `stepFailed` and `runFailed` both map to failures.
- [ ] **Step 2:** Implement `RunNotifier` + `NotificationPosting` + the tab (three toggles with per-kind descriptions, authorization-denied banner). Wire ownership + delegate in `RupuApp.swift`/`AppDelegate`.
- [ ] **Step 3:** `make macos-test` full suite + `make macos-build` green.
- [ ] **Step 4:** Commit: `feat(macos): notifications — firehose-driven UNUserNotificationCenter with per-kind prefs`

---

### Task 8: Menu bar extra

**Files:**
- Create: `apps/rupu-macos/RupuKit/Sources/RupuMenuBar/MenuBarStore.swift`
- Create: `apps/rupu-macos/RupuKit/Sources/RupuMenuBar/MenuBarView.swift`
- Modify: `apps/rupu-macos/RupuKit/Package.swift` (new `RupuMenuBar` target: deps RupuAPI, RupuStore, RupuDesign, RupuOverview; + test target)
- Modify: `apps/rupu-macos/App/RupuApp.swift` (`MenuBarExtra` scene)
- Modify: `apps/rupu-macos/project.yml` if the app target's dependency list enumerates modules (check first; regenerate with `make macos-gen` if changed)
- Test: `apps/rupu-macos/RupuKit/Tests/RupuMenuBarTests/MenuBarStoreTests.swift`

**Interfaces:**
- Consumes: `CPClient.dashboard(range:host:)` (`active: APIActiveCounts` — running/awaitingApproval/paused/pending), `CPClient.runs(offset:limit:host:)`, the existing `APIRunListRow → ActivityRow` mapping (find it in `ActivityStore` and reuse — do not duplicate the mapping), `deriveNeedsYou(rows:range:now:)` from RupuOverview, `PendingActions` gate machinery, `HostsFooterStore`'s poll-loop idiom.
- Produces:
  ```swift
  @MainActor @Observable public final class MenuBarStore {
      public private(set) var counts: APIActiveCounts?     // nil until first successful poll → view renders —
      public private(set) var needsYou: [NeedsYouItem]     // top 5, gates first (deriveNeedsYou already orders)
      public private(set) var overflow: Int
      public func activate(client: CPClient)                // 60s poll loop, HostsFooterStore idiom; polls dashboard + runs(limit: 50, host: "local") per tick
      public func deactivate()
      public func apply(counts: APIActiveCounts, rows: [ActivityRow], now: Date)  // pure seam
  }
  ```
- `MenuBarView(store:model:backend:openMainWindow:)`: attention layout — 4 stat tiles (running / awaiting / paused / pending; `—` pre-first-poll), needs-you list (top 5 + "N more in rupu" overflow line), rows deep-link (`model.navigate(.runDetail(...))` + `openMainWindow()`), gate rows get inline Approve/Reject via `PendingActions` (composite key `runID:stepID` — the Activity idiom), footer: **Open rupu** (`openMainWindow`), **New run** (navigate to activity + present launcher via the existing `LauncherStore` presentation seam — find how the toolbar's "+ New run" triggers it and reuse), **Settings…** (`SettingsLink` on macOS 14+).
- `MenuBarExtra` in `RupuApp`: label = the rupu glyph (existing asset) with a dot overlay when `counts?.awaitingApproval ?? 0 > 0`; style `.window`. The store activates on the scene's appear + backend-healthy (client-identity rebuild idiom) and deactivates on disappear — the poll must NOT run while the popover machinery is closed *unless* the attention dot needs it: the dot needs live data, so the store activates from app-level backend-healthy wiring (same place RunNotifier activates) and stays alive; doc-comment this deliberately (one lightweight 60s local-only poll).
- The badge dot is data the label closure reads from the store; `MenuBarExtra`'s label re-evaluates on `@Observable` changes.

**Steps:**

- [ ] **Step 1:** Failing store tests: `apply` maps counts + derives top-5/overflow (feed >5 needs-you rows); poll-loop idempotence (second `activate` doesn't double-poll — assert via stub hit count with generation tokens); failed poll keeps last counts.
- [ ] **Step 2:** Implement store + view + Package.swift target + scene wiring (+ `make macos-gen` if project.yml changed).
- [ ] **Step 3:** `make macos-test` full suite + `make macos-build` green.
- [ ] **Step 4:** Commit: `feat(macos): menu bar extra — live counts, needs-you triage, gate actions`

---

## Post-plan (controller)

Live GUI validation against the real CP (Settings all four tabs incl. a real global-raw save round-trip on a scratch key, notification fire on a real gate, menu bar counts + deep-link), checkpoint package to matt, final whole-branch review, PR. The three Phase 5B bug classes get explicit checks on the effective-config list and the needs-you menu list.
