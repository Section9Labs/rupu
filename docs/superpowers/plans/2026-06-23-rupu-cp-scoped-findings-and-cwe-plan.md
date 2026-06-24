# rupu-cp Scoped Findings + CWE Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development.

**Goal:** Scope the findings list by project (`ws_id`) and workflow (`workflow_name`), show a linked CWE chip + concern_id per finding, and make severity pills uniform width.

**Spec:** `docs/superpowers/specs/2026-06-23-rupu-cp-scoped-findings-and-cwe-design.md`

**Constraints (every task):** read-adapter (no `rupu-cli` dep in `rupu-cp`); no `any` in TS; static Tailwind only (severity via `SEVERITY_STYLE` — no dynamic class strings); recharts stays out of the main chunk (`grep -c recharts dist/assets/index-*.js` = 0); stage only specific changed files (`git add <paths>`, never `-A`, never `.rupu/*`); never run package-wide `cargo fmt`. Worktree runs Homebrew Rust 1.95 — `rupu-cp` is clean there, so `cargo test/clippy -p rupu-cp` are valid gates; ignore any `rupu-cli` red baseline.

---

### Task 1: Backend — workflow join + optional `ws_id`/`workflow` filters

**Files:** Modify `crates/rupu-cp/src/api/findings.rs`.

**Context:** `GET /api/findings` aggregates findings across workspaces with `FindingOut { #[serde(flatten)] record, ws_id, project, target_id }` and a per-severity `summary`, sorted critical→info then `declared_at` desc, built by a pure `build_response`. `AppState` exposes `s.run_store` (`rupu_orchestrator::runs::RunStore`); `RunStore::load(run_id) -> Result<RunRecord, RunStoreError>` and `RunRecord.workflow_name: String`. `FindingRecord.declared_by` is an `Attribution { run_id, model, surface }`.

- [ ] **Step 0 (verify the join):** Read where the coverage `report_finding` path stamps `Attribution.run_id` (search `rupu-coverage` for where `Attribution`/`declared_by` is constructed). Confirm it is the orchestrator run id that `RunStore` is keyed by. If it is NOT (synthetic/agent-local id), STOP and report — the workflow filter would be dead. If confirmed, proceed and note it in the final report.
- [ ] **Step 1: Write failing tests.** Add tests asserting: (a) the pure join/filter helper attaches `workflow_name` from a `run_id → workflow_name` map; (b) filtering by `ws_id` keeps only that workspace's findings and the summary reflects the filtered set; (c) filtering by `workflow` keeps only findings whose resolved `workflow_name` matches; (d) no filter → all. Factor a pure function (e.g. `fn scope(findings: Vec<FindingOut>, q: &FindingsQuery) -> FindingsResponse`) so this is testable without a server / without RunStore (pass `workflow_name` pre-attached on the `FindingOut`s in the test).
- [ ] **Step 2: Run `cargo test -p rupu-cp findings`, confirm failure.**
- [ ] **Step 3: Implement.** Add `workflow_name: Option<String>` to `FindingOut`. Add `struct FindingsQuery { ws_id: Option<String>, workflow: Option<String> }` (plain `Option<String>` fields — NOT `#[serde(flatten)]`). In the handler: after gathering findings, collect DISTINCT `run_id`s, `RunStore::load` each once into a `HashMap<String,String>` (load error / `NotFound` → skip, leave `None`), attach `workflow_name`. Then apply the `ws_id`/`workflow` filters and build the summary over the filtered set (reuse/extend `build_response`). Handler signature takes `Query<FindingsQuery>`.
- [ ] **Step 4: Run `cargo test -p rupu-cp` + `cargo clippy -p rupu-cp --all-targets`, confirm green/clean.**
- [ ] **Step 5: Commit.** `git add crates/rupu-cp/src/api/findings.rs` → `feat(cp): /api/findings workflow join + ws_id/workflow filters`.

---

### Task 2: Frontend — CWE chip, uniform severity pill, API client params

**Files:** Create `crates/rupu-cp/web/src/lib/cwe.ts`; Modify `crates/rupu-cp/web/src/components/findings/FindingRow.tsx`, `crates/rupu-cp/web/src/lib/api.ts`; Test `crates/rupu-cp/web/src/lib/cwe.test.ts`.

**Context:** `FindingRow` (`components/findings/FindingRow.tsx`) renders the severity pill + summary + location + optional concern_id + optional provenance chip. The pill class string is around `'shrink-0 inline-flex items-center rounded px-2 py-0.5 text-[11px] font-semibold uppercase tracking-wide ring-1 mt-0.5'` + `s.pill`. `lib/api.ts` has `getFindings(): Promise<FindingsResponse>`, `FindingOut extends FindingRecord { ws_id; project; target_id }`, and `FindingRecord` (with `concern_id?`, `evidence.references: string[]`).

- [ ] **Step 1: Write failing tests** for `cweFromFinding(finding): { id: string; url: string } | null` in `cwe.test.ts`: (a) `concern_id: 'cwe-top25-2023:cwe-787-out-of-bounds-write'` → `{ id: 'CWE-787', url: 'https://cwe.mitre.org/data/definitions/787.html' }`; (b) no `concern_id` but `evidence.references: ['https://cwe.mitre.org/data/definitions/798.html']` → `CWE-798`; (c) neither → `null`; (d) a non-CWE concern_id (e.g. `owasp-top10-2021:a01-...`) with no CWE reference → `null`.
- [ ] **Step 2: Run `npm test -- --run cwe`, confirm failure.**
- [ ] **Step 3: Implement.** `lib/cwe.ts`: `cweFromFinding` — match `concern_id` with `/cwe[-_]?(\d+)/i`; else scan `evidence?.references` for `/cwe\.mitre\.org\/data\/definitions\/(\d+)/i`; build `{ id: 'CWE-'+n, url }`. In `FindingRow.tsx`: render a linked chip (`<a href={cwe.url} target="_blank" rel="noreferrer" className="...static neutral chip...">{cwe.id}</a>`) next to the location when non-null; keep `concern_id` rendering as-is. Add `min-w-[72px] justify-center text-center` to the severity pill class string. In `lib/api.ts`: change `getFindings` to `getFindings(opts?: { wsId?: string; workflow?: string })` building a query string (`ws_id`, `workflow`) when provided; add `workflow_name?: string | null` to `FindingOut`.
- [ ] **Step 4: `npm test -- --run` (full) + `npm run build` (strict)** — green + exit 0; `grep -c recharts dist/assets/index-*.js` = 0.
- [ ] **Step 5: Commit.** `git add crates/rupu-cp/web/src/lib/cwe.ts crates/rupu-cp/web/src/lib/cwe.test.ts crates/rupu-cp/web/src/components/findings/FindingRow.tsx crates/rupu-cp/web/src/lib/api.ts` → `feat(cp/web): CWE chip + uniform severity pill + scoped getFindings`.

---

### Task 3: Project detail — scoped findings section

**Files:** Modify `crates/rupu-cp/web/src/pages/ProjectDetail.tsx`.

**Context:** Route `/projects/:wsId` (`ProjectDetail` reads `useParams` `wsId`). Depends on Task 2's `getFindings({ wsId })`. Reuse `FindingMetrics` + `FindingRow` (from `components/findings/`). Match the page's existing section chrome (it already shows a coverage rollup).

- [ ] **Step 1: Implement.** Add a Findings section: on mount (cancel-guarded, matching the page's existing fetch pattern), call `getFindings({ wsId })`; render `<FindingMetrics summary={resp.summary} />` (static) + the `<FindingRow>` list (pass `project`/`targetId` provenance per row). Loading/empty/error states consistent with the page; empty → a small "No findings" note. No `any`; static Tailwind.
- [ ] **Step 2: `npm test -- --run` + `npm run build`** — green + exit 0.
- [ ] **Step 3: Commit.** `git add crates/rupu-cp/web/src/pages/ProjectDetail.tsx` → `feat(cp/web): findings section on project detail (scoped by ws_id)`.

---

### Task 4: Workflow detail — scoped findings section

**Files:** Modify `crates/rupu-cp/web/src/pages/WorkflowDetail.tsx`.

**Context:** Route `/workflows/:name` (`WorkflowDetail` reads `useParams` `name`; renders header + steps + raw YAML, no findings today). Depends on Task 2's `getFindings({ workflow })`.

- [ ] **Step 1: Implement.** Add a Findings section mirroring Task 3 but calling `getFindings({ workflow: name })`; render `<FindingMetrics>` + `<FindingRow>` list. If the workflow has no findings (incl. the case where the run_id→workflow join resolved nothing), show the "No findings" empty state. No `any`; static Tailwind; consistent chrome.
- [ ] **Step 2: `npm test -- --run` + `npm run build`** — green + exit 0; recharts grep = 0; report the main chunk size.
- [ ] **Step 3: Commit.** `git add crates/rupu-cp/web/src/pages/WorkflowDetail.tsx` → `feat(cp/web): findings section on workflow detail (scoped by workflow)`.

---

### Final verification (after all tasks)
- `cargo test -p rupu-cp` + `cargo clippy -p rupu-cp --all-targets` clean.
- `npm test -- --run` full green; `npm run build` strict exit 0; recharts out of main chunk.
- Final whole-branch review (spec compliance + integration: Rust↔TS contract, the run_id join actually resolving), then hand to matt for visual validation.
