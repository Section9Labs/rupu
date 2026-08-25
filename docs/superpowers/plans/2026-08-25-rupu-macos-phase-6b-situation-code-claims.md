# rupu.app macOS Phase 6B — Situation Room, Code Viewers, Claims Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The ambient surfaces — a fullscreen Situation Room scene, project/run code viewers, and the autoflow claims table — completing Phase 6 and the umbrella's API parity ledger.

**Architecture:** New `RupuAPI` surfaces (claims, project code, run source/AST) → stores in RupuStore → three UI deliveries: a Runs/Claims sub-toggle in Activity's autoflows kind, a Code tab in Project detail + inline source/CST previews in the run transcript, and a new `RupuSituation` module whose pure card/roster/vitals derivations are ports of the web's shipped `lib/situationRoom/{cards,roster}.ts` (the algorithms are the contract; the React layer is not ported).

**Tech Stack:** SwiftUI (secondary fullscreen `Window` scene), Swift Testing, Rust fixture emission.

**Spec:** `docs/superpowers/specs/2026-08-25-rupu-macos-phase-6-ambient-design.md` (§4–§7)

## Global Constraints

Same as Plan 6A — binding verbatim:
- Any Swift test touching a SwiftUI `View`-type member MUST be `@Test @MainActor`.
- Test-stub statics use generation-token isolation.
- EVERY task ends with the FULL `make macos-test` suite green (+ `cargo test -p rupu-cp` for Rust-touching tasks). A red test is never "owned by a later task".
- Timing tests condition-poll with WIDE margins. Per-file `rustfmt` only. No third-party Swift deps.
- Citations verified against source before written. Honest UI (`—` nulls, truthful truncation footers, no dead controls).
- Store idioms: `@MainActor @Observable`, generation guards, client-identity rebuild, composite ForEach identity wherever wire ids aren't globally unique, lazy-tab task-ids include parent-block resolution.
- Implementers never dispatch subagents. Never bare `git stash` / `git stash pop`.

---

### Task 1: Fixtures — claims, code, source, ast (Rust)

**Files:**
- Create: `apps/rupu-macos/Fixtures/{autoflow_claims,code_tree,code_file,code_files,run_source,run_ast}.json` (generated)
- Modify: `crates/rupu-cp/tests/macos_fixtures.rs`
- Modify (visibility only, if needed): `crates/rupu-cp/src/api/{autoflow_claims,code,source}.rs`

**Interfaces:**
- Produces six fixtures from the REAL serde types:
  - `autoflow_claims.json`: `Vec<ClaimRow>` (`api/autoflow_claims.rs` — already `pub(crate)`), ≥3 rows covering: an `await_human` status with `last_error` + `pr_url` set; an active claim with `claim_owner` + `lease_expires_at`; a row with every Option field `None` (honest-`—` coverage).
  - `code_tree.json`: `TreeResult` (`api/code.rs`) with `parent: Some(..)`, dir + file entries.
  - `code_file.json`: `FileContent` — one available file with `lines`/`language`/`total_lines`, and note: emit a SECOND unavailable variant inside the same fixture only if the type is a single struct — otherwise cover `available:false, reason:Some` via `run_source.json`.
  - `code_files.json`: `FileListResult` with `truncated: true`.
  - `run_source.json`: `SourceSlice` (`api/source.rs`) — one available slice (with `target_line`, numbered `lines`) — plus a separate unavailable case: make the fixture a two-element JSON array `[available, unavailable]` with `reason: Some("REMOTE_NOT_SUPPORTED")` on the second (fixture tests already established multi-case array fixtures — follow the existing pattern; if none exists, two separate files `run_source.json` / `run_source_unavailable.json`).
  - `run_ast.json`: `AstResponse` with a small real `rupu_ast::AstNode` tree (≥3 levels), `truncated: Some(false)`.
- Statuses in `ClaimRow.status` are the snake_case `ClaimStatus` strings — enumerate the real variants from `rupu_workspace`'s `ClaimStatus` (read it; do not invent strings).

**Steps:**

- [ ] **Step 1:** Read the three API files + `rupu_workspace::AutoflowClaimRecord`/`ClaimStatus` + `rupu_ast::AstNode`; read the existing fixture rig in `macos_fixtures.rs`.
- [ ] **Step 2:** Add the fixture tests (one per file, real-type construction; hoist visibility to `pub(crate)` where a type is private — handlers untouched). Regenerate, then full `cargo test -p rupu-cp` green.
- [ ] **Step 3:** `make macos-test` full suite green (nothing consumes the fixtures yet).
- [ ] **Step 4:** Commit: `test(macos): claims/code/source/ast fixtures from rupu-cp serde types`

---

### Task 2: RupuAPI — claims, code, source surfaces

**Files:**
- Create: `apps/rupu-macos/RupuKit/Sources/RupuAPI/{ClaimsModels,CodeModels,SourceModels}.swift`
- Modify: `apps/rupu-macos/RupuKit/Sources/RupuAPI/CPClient.swift`
- Test: `apps/rupu-macos/RupuKit/Tests/RupuAPITests/{ClaimsModels,CodeModels,SourceModels}Tests.swift`

**Interfaces:**
- Produces (snake_case CodingKeys throughout; all `Decodable, Sendable`, `Equatable` where cheap):
  ```swift
  public struct APIClaimRow: ...  // issue_ref, issue_display_ref?, repo_ref, issue_title?, issue_url?, workflow,
                                   // status (String — untyped, matching the app's existing status-string idiom),
                                   // last_run_id?, last_error?, last_summary?, pr_url?, claim_owner?,
                                   // lease_expires_at?, updated_at
  public struct APITreeEntry: ... // name, path, kind
  public struct APITreeResult: ...// path, parent?, entries
  public struct APIFileContent: ...  // available, path?, language?, total_lines?, lines? [APISourceLine], reason?
  public struct APIFileList: ...     // files [String], truncated
  public struct APISourceLine: ...   // n, text
  public struct APISourceSlice: ...  // available, path?, language?, start_line?, end_line?, target_line?, total_lines?, lines?, reason?
  public struct APIAstNode: Decodable, Sendable { ... }  // recursive — mirror rupu_ast::AstNode's serde shape exactly (read the Rust type; children array recursion)
  public struct APIAstResponse: ...  // available, language?, root? APIAstNode, truncated?, reason?
  // CPClient:
  public func autoflowClaims() async throws -> [APIClaimRow]                      // GET /api/autoflows/claims
  public func releaseClaim(issueRef: String) async throws -> Bool                 // POST .../release {"issue_ref"} → {"released": Bool}
  public func requeueClaim(issueRef: String) async throws                         // POST .../requeue {"issue_ref"} (not_before unused — no UI for deferral; doc-comment)
  public func projectTree(wsID: String, path: String) async throws -> APITreeResult        // GET /api/projects/:ws_id/tree?path=
  public func projectFile(wsID: String, path: String) async throws -> APIFileContent       // GET .../source?path=
  public func projectFiles(wsID: String) async throws -> APIFileList                       // GET .../files
  public func runSource(id: String, path: String, line: Int, context: Int, host: String?) async throws -> APISourceSlice  // GET /api/runs/:id/source
  public func runAst(id: String, path: String, line: Int, col: Int, host: String?) async throws -> APIAstResponse         // GET /api/runs/:id/ast
  ```
- Read the actual query-param names/defaults from `SourceQuery`/`AstQuery`/`TreeQuery`/the files route before writing the URLs — do not guess (`context` has a serde default; match it).

**Steps:**

- [ ] **Step 1:** Failing decode tests from each Task 1 fixture (bundle-load idiom): claims all-None row decodes with nils; tree parent survives; ast recursion depth ≥3 reachable; source unavailable case carries `reason == "REMOTE_NOT_SUPPORTED"`.
- [ ] **Step 2:** Implement models + client methods (mutation POSTs follow the existing idiom; error surface unchanged).
- [ ] **Step 3:** `make macos-test` full suite green.
- [ ] **Step 4:** Commit: `feat(macos): RupuAPI claims/code/source surfaces`

---

### Task 3: Claims — store + Activity sub-toggle

**Files:**
- Create: `apps/rupu-macos/RupuKit/Sources/RupuStore/ClaimsStore.swift`
- Create: `apps/rupu-macos/RupuKit/Sources/RupuActivity/ClaimsTable.swift`
- Modify: `apps/rupu-macos/RupuKit/Sources/RupuActivity/ActivityScreen.swift` (autoflows kind gains a Runs/Claims segmented sub-toggle)
- Test: `apps/rupu-macos/RupuKit/Tests/RupuStoreTests/ClaimsStoreTests.swift`, `.../RupuActivityTests/ClaimsTableTests.swift`

**Interfaces:**
- Consumes: `CPClient.autoflowClaims/releaseClaim/requeueClaim`; `PendingActions` (existing autoflow `ActionKey` machinery — read it; claims need their own key case or a composite `issue_ref:verb` — follow whichever the existing enum shape makes natural, extending `ActionKey` if needed).
- Produces: `ClaimsStore` (`BlockState<[APIClaimRow]>`-style load + `release(issueRef:)`/`requeue(issueRef:)` that: mark pending → call → on success re-load → confirm; these endpoints respond synchronously, so confirmation comes from the response + refreshed list — doc-comment the contrast with the run-mutation pending-state contract). `ClaimsTable`: columns issue (display_ref ?? issue_ref, linking `issue_url` when present), repo, workflow, status pill, owner/lease (`—` when nil), last error/summary (truncated single line), PR link chip when present, updated_at (via the tolerant `ActivityRow.parseISO` home for any date rendering — never lexicographic). Row actions: **Release** (confirm-first `confirmationDialog` — release is destructive-ish: it forgets the claim; idempotent per server), **Requeue** (no confirm — it only enqueues a wake). Sub-toggle state is screen-local (`@State`), defaulting to Runs; Claims tab lazy-loads on first selection (task-id includes the toggle), local-CP-only data noted in a footer line.
- ForEach identity: `issue_ref` (globally unique per store contract — verify by reading `AutoflowClaimStore.list` dedup; if not guaranteed unique, composite with `updated_at`).

**Steps:**

- [ ] **Step 1:** Failing store tests: load from fixture; release success re-loads (stub asserts second GET); release of untracked (`released:false`) still confirms + re-loads (idempotent contract); requeue error surfaces without dropping rows.
- [ ] **Step 2:** Implement store + table + toggle wiring; `@Test @MainActor` view tests for the nil-heavy row rendering `—`s and the confirm-dialog presence flag.
- [ ] **Step 3:** `make macos-test` full suite green.
- [ ] **Step 4:** Commit: `feat(macos): autoflow claims — Runs/Claims toggle, release/requeue`

---

### Task 4: Project Code tab

**Files:**
- Create: `apps/rupu-macos/RupuKit/Sources/RupuStore/CodeStore.swift`
- Create: `apps/rupu-macos/RupuKit/Sources/RupuProjects/CodeTab.swift` (tree column + viewer pane + filter field)
- Modify: `apps/rupu-macos/RupuKit/Sources/RupuProjects/ProjectDetailScreen.swift` (Code tab lands in the deferred slot)
- Test: `apps/rupu-macos/RupuKit/Tests/RupuStoreTests/CodeStoreTests.swift`, `.../RupuProjectsTests/CodeTabTests.swift`

**Interfaces:**
- Consumes: `CPClient.projectTree/projectFile/projectFiles`; ProjectDetail's existing tab enum + lazy-tab task-id discipline (id includes parent detail resolution — the PR #501 class).
- Produces: `CodeStore` — `tree: BlockState<APITreeResult>` for the current dir path, `file: BlockState<APIFileContent>?` for the selected file, `filter: BlockState<APIFileList>?` fetched once on first filter keystroke; `navigate(path:)`/`open(path:)`/`loadFilter()` all generation-guarded (rapid dir clicks must not interleave — single generation covers tree+file). `CodeTab`: left tree column (dirs first, `..` row from `parent`, folder/file glyphs), right viewer (line numbers from `APISourceLine.n`, mono, language label chip, `available:false` renders the `reason` honestly — "binary file" / size cap verbatim), top filter field over `files` (client-side contains-match, truncation footer "+ more (list truncated)" when `truncated`). Selection state screen-local; no editing affordances (viewer only — doc-comment).
- ForEach identity: entry `path` (unique within a listing by construction).

**Steps:**

- [ ] **Step 1:** Failing store tests: tree load; open file; generation guard (navigate mid-flight drops stale); filter loads once and is reused.
- [ ] **Step 2:** Implement store + tab; wire into ProjectDetail (task-id carries `detail.value != nil` + tab selection).
- [ ] **Step 3:** `make macos-test` full suite green.
- [ ] **Step 4:** Commit: `feat(macos): project Code tab — tree, viewer, filter`

---

### Task 5: Run transcript source + CST previews

**Files:**
- Modify: `apps/rupu-macos/RupuKit/Sources/RupuRunDetail/` (the transcript tool-card views — find the grep/ast_grep result rendering; add expandable preview sections)
- Create: `apps/rupu-macos/RupuKit/Sources/RupuRunDetail/{SourcePreview,AstTreeView}.swift`
- Create: `apps/rupu-macos/RupuKit/Sources/RupuStore/SourcePreviewStore.swift` (tiny per-card fetch cache)
- Test: `apps/rupu-macos/RupuKit/Tests/RupuRunDetailTests/SourcePreviewTests.swift`

**Interfaces:**
- Consumes: `CPClient.runSource/runAst`; the transcript feed's existing tool-card model (read how tool results carry `file:line` matches — mirror the web's mount points: `crates/rupu-cp/web/src` `ToolCard`-adjacent components show where grep/ast_grep matches become source links; port the mount decision, not the React internals).
- Produces: on a match row's expand: `SourcePreview` fetches `runSource(id:path:line:context:host:)` (host from the run's route context — remote runs get the server's `REMOTE_NOT_SUPPORTED` reason rendered honestly, never an infinite spinner) and renders the numbered slice with the `target_line` highlighted. ast_grep matches additionally offer a CST disclosure → `AstTreeView` renders `runAst`'s recursive tree (indented `OutlineGroup` or recursive view, matched node highlighted via the response's location fields — read `AstResponse`/`AstNode` for what marks the match; `truncated == true` renders a truthful footer). Fetches are lazy (on expand only), cached per `(path,line)` in `SourcePreviewStore` for the screen's lifetime, generation-guarded on run identity.
- Failure states: slice `available:false` → reason text; HTTP error → compact retry affordance (this store is new — give it the retry the older blocks defer).

**Steps:**

- [ ] **Step 1:** Read the web's usage of `/api/runs/:id/source|ast` (grep `runs/` + `source?` in `crates/rupu-cp/web/src`) to confirm which tool kinds mount previews and what params they pass; record findings in the report.
- [ ] **Step 2:** Failing tests: store caches per key (second expand = no second hit); unavailable reason surfaces; run-identity change flushes cache.
- [ ] **Step 3:** Implement store + views + tool-card wiring.
- [ ] **Step 4:** `make macos-test` full suite green.
- [ ] **Step 5:** Commit: `feat(macos): transcript source slices + ast_grep CST previews`

---

### Task 6: Situation Room — pure derivations

**Files:**
- Create: `apps/rupu-macos/RupuKit/Sources/RupuSituation/{StreamCards,Roster,Vitals}.swift`
- Modify: `apps/rupu-macos/RupuKit/Package.swift` (new `RupuSituation` target: deps RupuAPI, RupuDesign; + test target)
- Test: `apps/rupu-macos/RupuKit/Tests/RupuSituationTests/{StreamCardsTests,RosterTests}.swift`

**Interfaces:**
- Consumes: `CPEvent`, `APIFinding` (RupuAPI findings models), `Severity` (RupuDesign).
- Produces Swift ports of `crates/rupu-cp/web/src/lib/situationRoom/cards.ts` and `roster.ts` — **the web files are the line-by-line contract; read them fully first**:
  ```swift
  public struct StreamCard: Equatable, Sendable { // key, ts (ms), form, group, accent, badge, title, detail?,
                                                   // runID?, projectName?, stepID?, agent?, severity?, fileRef?, code?,
                                                   // wsID?, filePath?, fileLine?, permalink?, approvable?
  }
  public enum CardGroup { case finding, await_, error, activity }
  public func cardForEvent(_ e: CPEvent, ts: Date?) -> StreamCard?      // port of cards.ts event mapping (which kinds render, which are skipped)
  public func cardForFinding(_ f: APIFinding) -> StreamCard             // severity accent, evidence excerpt (never fabricated)
  public func mergeStream(_ cards: [StreamCard], max: Int) -> [StreamCard]  // newest-first, content-identity dedup EXCLUDING ts/pos (port the web's dedup key exactly)
  public struct RosterEntry: ... ; public func foldRoster(...) -> [RosterEntry]  // roster.ts port: per-project fold with terminal-status reconciliation
  public struct Vitals: ... // active/awaiting passthrough + events-per-minute over a session-local ring buffer
  ```
- Port the web's test tables (`cards.test.ts`, `roster.test.ts`) as the Swift test tables — same inputs, same expected outputs, adapted to `CPEvent`'s typed cases. Where `CPEvent.unknown` arrives, the card mapping returns nil (web's `isKnownRunEvent` gate).

**Steps:**

- [ ] **Step 1:** Read all of `cards.ts`/`roster.ts` + both test files. Write the ported failing test tables.
- [ ] **Step 2:** Implement the pure functions until the tables pass.
- [ ] **Step 3:** `make macos-test` full suite green.
- [ ] **Step 4:** Commit: `feat(macos): RupuSituation — card/roster/vitals derivations (web algorithm port)`

---

### Task 7: Situation Room scene

**Files:**
- Create: `apps/rupu-macos/RupuKit/Sources/RupuSituation/{SituationRoomScreen,PulseStrip,EventStreamColumn,RosterColumn}.swift`
- Create: `apps/rupu-macos/RupuKit/Sources/RupuStore/SituationStore.swift`
- Modify: `apps/rupu-macos/App/RupuApp.swift` (secondary `Window("Situation Room", id: "situation")` scene; View menu command "Enter Situation Room" via `openWindow`; the window enters fullscreen on appear)
- Modify: `apps/rupu-macos/RupuKit/Package.swift` (RupuSituation gains RupuStore dep for the screen layer)
- Test: `apps/rupu-macos/RupuKit/Tests/RupuStoreTests/SituationStoreTests.swift`

**Interfaces:**
- Consumes: Task 6's pure derivations; `BackendController.makeFirehoseStream` (own independent stream — established seam); `CPClient.recentEvents` (history backfill), `findings()` (REST merge — the web merges findings into the stream by `declared_at`; poll every 60s), `dashboard(range:host:)` (vitals counts), `projects()` (run→workspace label resolution, the web page's `projectName` resolution); `PendingActions` for await-card approve/reject; `AppModel.navigate` for deep links (deep-linking from the fullscreen scene also fronts the main window via `openWindow`).
- Produces: `SituationStore` — activate: history backfill + live stream fold into a capped card list (`mergeStream`, cap matching the web's), findings poll merged by ts, roster fold refreshed on each batch, vitals ring buffer; generation-guarded on client identity; deactivate on scene close (stream must not outlive the window). Screen: **dark always** (`.preferredColorScheme(.dark)` on the scene root, deliberate spec exception to the theme system — doc-comment), three-region layout (PulseStrip top, stream center scrolling newest-first with the group filter chips ported from the web page: All/Findings/Needs you/Errors, roster right), await cards with inline Approve/Reject, finding cards with severity stripe + code excerpt (mono, contained), everything windowed (LazyVStack; the Phase 5B eager-container lesson).
- ForEach identity: `StreamCard.key` (the dedup key IS the identity — collisions already merged).

**Steps:**

- [ ] **Step 1:** Failing store tests: backfill + live event fold dedups (same event via history AND stream renders once); findings merge orders by ts; cap enforced; deactivate cancels the stream (stub-level assert).
- [ ] **Step 2:** Implement store, screen, scene + menu command.
- [ ] **Step 3:** `make macos-test` full suite + `make macos-build` green.
- [ ] **Step 4:** Commit: `feat(macos): Situation Room — fullscreen live wall (stream, roster, vitals)`

---

## Post-plan (controller)

Live GUI validation (Situation Room against the real firehose with a live run, claims against real claim records, Code tab on the rupu workspace itself, a transcript source preview on a real grep result), umbrella §8 parity-ledger update (Phase 6 row + dispositions from spec §7), CLAUDE.md read-first + module-map update, checkpoint package, final review, PR.
