# Finding reports — Plan 2: report API + web UI

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The control plane shows a full-profile finding as a structured report. That means a finding detail page, a triage card in the findings tables, a tabbed card in the Code tab, and artifacts served by a safe endpoint. Workflow authors can also set `findings_profile` in the web workflow editor.

**Architecture:**
- rupu-coverage gains a pure `summarize(&FindingReport)` (a completeness count plus the fields a list row needs) and a checked blob-path accessor.
- rupu-cp's list endpoint stops shipping the heavy report and returns a small `report_summary` instead.
- New `GET /api/findings/:id` returns the full record plus per-claim staleness.
- New `GET /api/findings/:id/artifacts/:sha256` serves an artifact that the finding itself references, as plain text or as an attachment, never as HTML.
- The web app gets typed report models, section renderers, a `/findings/:id` page, triage and inline cards, and editor fields.

**Tech Stack:** Rust (axum 0.7, serde, sha2, tokio, tokio-util), React 18 + TypeScript + Tailwind, vitest + @testing-library/react.

**Spec:** `docs/superpowers/specs/2026-09-29-rupu-finding-reports-design.md` (§ CP API, § Rendering → Web). Builds on Plan 1 (`docs/superpowers/plans/2026-09-29-rupu-finding-reports-plan-1-contract.md`).

## Global Constraints

- **The macOS app is out of scope** (deprecated). Write no Swift and don't run `make macos-test`/`macos-build`. Do run `make macos-fixtures` whenever a CP serde shape changes, because `cargo test -p rupu-cp` enforces the JSON fixtures.
- Workspace deps only; `#![deny(clippy::all)]`; `unsafe_code` forbidden. `rupu-coverage` must not depend on `rupu-config`/`rupu-cli`/`rupu-cp`.
- Only run rustfmt on leaf files you change. Never rustfmt `lib.rs`/`mod.rs`/`main.rs`; never `cargo fmt`.
- Web: no new npm dependencies. Tests start with `// @vitest-environment jsdom`, import `@testing-library/jest-dom/vitest`, and call `afterEach(cleanup)`. Mock the API with `vi.spyOn(api, '…')`, not msw. Run with `cd crates/rupu-cp/web && npx vitest run <path>`; type-check with `npx tsc --noEmit -p .`.
- Tailwind tokens only (`text-ink`, `text-ink-dim`, `text-ink-mute`, `bg-panel`, `bg-surface`, `ring-border`, `border-border`, `text-ui`/`text-note`/`text-meta`/`text-lead`, `sev-*`, `ok`/`err`/`warn` + `-bg`, `brand-50/100/500/600/700`).
- Artifacts are never rendered as HTML. Serve text as `text/plain; charset=utf-8` with `X-Content-Type-Options: nosniff`, and binary as `application/octet-stream` with `Content-Disposition: attachment`.
- An artifact is only served when the requested sha256 is 64 lowercase hex digits **and** the finding's `report.artifacts` lists it.
- Exports (Markdown/HTML/PDF) are Plan 3. Add no export buttons in this plan.
- There is one pre-existing web test failure on main (`src/pages/runs/AutoflowRuns.columnOrder.test.tsx`), tracked separately. Ignore it.
- Every change goes through a feature branch plus PR. Rebase on `origin/main` before each task.

---

### Task 1: `summarize()` — completeness and list-row summary (rupu-coverage)

**Files:**
- Create: `crates/rupu-coverage/src/report/summary.rs`
- Modify: `crates/rupu-coverage/src/report/mod.rs` (module + re-exports), `crates/rupu-coverage/src/report/artifacts.rs` (make `sha256_file` `pub`, add `blob_path_checked`)

**Interfaces:**
- Produces:
  - `pub struct Completeness { pub filled: usize, pub total: usize, pub gaps: Vec<&'static str> }` (Serialize)
  - `pub struct ReportSummary { pub owner: String, pub product: String, pub cwe: Vec<String>, pub root_cause: String, pub chain: Vec<String>, pub completeness: Completeness, pub has_poc: bool, pub verification_status: Option<VerificationStatus> }` (Serialize)
  - `pub fn completeness(r: &FindingReport) -> Completeness`
  - `pub fn summarize(r: &FindingReport) -> ReportSummary`
  - `pub fn sha256_file(p: &Path) -> std::io::Result<String>` (made public)
  - `ArtifactStore::blob_path_checked(&self, sha256: &str) -> Option<PathBuf>` (None unless `sha256` is 64 lowercase hex)

The gap-able fields, `total = 11`, are, in order: `owner`, `product`, `affected_component`, `source_repository`, `tickets`, `cvss_v3`, `attack_vector`, `call_chain`, `recommended_patch`, `ci_cd_detection`, `regression_test`. A field is a gap when:
- it is the `Unknown` sentinel (all string fields, plus `tickets`), or
- it is a `Not Provided — …` sentinel (`call_chain`, patch, CI, regression).

`None Provided` (tickets), `Not Applicable` (source_repository) and `None` (cross references) are real answers, not gaps.

- [ ] **Step 1: Write the failing tests** at the bottom of the new `summary.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::{FindingReport, OrSentinel};

    fn fixture() -> FindingReport {
        serde_json::from_str(include_str!("../../tests/fixtures/finding_report/valid_full.json")).unwrap()
    }

    #[test]
    fn fixture_gaps_are_owner_and_cvss() {
        // valid_full.json has owner "Unknown" and cvss_v3 "Unknown".
        let c = completeness(&fixture());
        assert_eq!(c.total, 11);
        assert_eq!(c.gaps, vec!["owner", "cvss_v3"]);
        assert_eq!(c.filled, 9);
    }

    #[test]
    fn not_provided_sections_are_gaps_but_none_answers_are_not() {
        let mut r = fixture();
        r.regression_test = OrSentinel::Sentinel("Not Provided — needs hardware".into());
        r.ownership.source_repository = "Not Applicable".into();
        r.tickets = OrSentinel::Sentinel("None Provided".into());
        let c = completeness(&r);
        assert!(c.gaps.contains(&"regression_test"));
        assert!(!c.gaps.contains(&"source_repository"));
        assert!(!c.gaps.contains(&"tickets"));
    }

    #[test]
    fn unknown_tickets_is_a_gap() {
        let mut r = fixture();
        r.tickets = OrSentinel::Sentinel("Unknown".into());
        assert!(completeness(&r).gaps.contains(&"tickets"));
    }

    #[test]
    fn summarize_carries_list_row_fields() {
        let s = summarize(&fixture());
        assert_eq!(s.owner, "Unknown");
        assert_eq!(s.product, "Notebin (sample app)");
        assert_eq!(s.cwe, vec!["CWE-639".to_string(), "CWE-862".to_string()]);
        assert!(s.root_cause.contains("find_by_id"));
        assert_eq!(s.chain.len(), 3);
        assert!(!s.has_poc);
        assert_eq!(s.verification_status, None);
    }
}
```

Add to `artifacts.rs`'s test module:

```rust
    #[test]
    fn blob_path_checked_rejects_non_hex_and_wrong_length() {
        let s = ArtifactStore::new("/tmp/store");
        assert!(s.blob_path_checked("").is_none());
        assert!(s.blob_path_checked("../etc/passwd").is_none());
        assert!(s.blob_path_checked(&"A".repeat(64)).is_none()); // uppercase
        assert!(s.blob_path_checked(&"a".repeat(63)).is_none());
        let ok = "0123456789abcdef".repeat(4);
        assert_eq!(
            s.blob_path_checked(&ok).unwrap(),
            std::path::PathBuf::from("/tmp/store").join("01").join(&ok)
        );
    }
```

- [ ] **Step 2: Run the tests and check they fail**

Run: `cargo test -p rupu-coverage report::summary report::artifacts::tests::blob_path_checked`
Expected: compile errors (the module and the functions don't exist yet).

- [ ] **Step 3: Implement**

Put this in `summary.rs`, above the tests:

```rust
//! Derived views of a finding report for list rows and completeness meters.
//! Pure functions over `FindingReport`, so the CP and exports compute them
//! identically.

use crate::report::types::{FindingReport, OrSentinel, VerificationStatus, NOT_PROVIDED_PREFIX};
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Completeness {
    pub filled: usize,
    pub total: usize,
    /// Field names whose value is an `Unknown` / `Not Provided — …` sentinel,
    /// in report order.
    pub gaps: Vec<&'static str>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReportSummary {
    pub owner: String,
    pub product: String,
    pub cwe: Vec<String>,
    pub root_cause: String,
    /// Call-chain hop labels, source → sink; empty when the chain is a sentinel.
    pub chain: Vec<String>,
    pub completeness: Completeness,
    pub has_poc: bool,
    pub verification_status: Option<VerificationStatus>,
}

fn is_unknown(s: &str) -> bool {
    s.trim() == "Unknown"
}

fn is_not_provided<T>(v: &OrSentinel<T>) -> bool {
    matches!(v, OrSentinel::Sentinel(s) if s.starts_with(NOT_PROVIDED_PREFIX))
}

pub fn completeness(r: &FindingReport) -> Completeness {
    let checks: [(&'static str, bool); 11] = [
        ("owner", is_unknown(&r.ownership.owner)),
        ("product", is_unknown(&r.ownership.product)),
        ("affected_component", is_unknown(&r.ownership.affected_component)),
        ("source_repository", is_unknown(&r.ownership.source_repository)),
        ("tickets", matches!(&r.tickets, OrSentinel::Sentinel(s) if is_unknown(s))),
        ("cvss_v3", is_unknown(&r.rating.cvss_v3)),
        ("attack_vector", is_unknown(&r.attack_vector)),
        ("call_chain", is_not_provided(&r.call_chain)),
        ("recommended_patch", is_not_provided(&r.recommended_patch)),
        ("ci_cd_detection", is_not_provided(&r.ci_cd_detection)),
        ("regression_test", is_not_provided(&r.regression_test)),
    ];
    let gaps: Vec<&'static str> = checks.iter().filter(|(_, gap)| *gap).map(|(n, _)| *n).collect();
    Completeness { filled: checks.len() - gaps.len(), total: checks.len(), gaps }
}

pub fn summarize(r: &FindingReport) -> ReportSummary {
    let chain = match &r.call_chain {
        OrSentinel::Value(hops) => hops.iter().map(|h| h.label.clone()).collect(),
        OrSentinel::Sentinel(_) => Vec::new(),
    };
    ReportSummary {
        owner: r.ownership.owner.clone(),
        product: r.ownership.product.clone(),
        cwe: r.cwe.clone(),
        root_cause: r.root_cause.clone(),
        chain,
        completeness: completeness(r),
        has_poc: !r.artifacts.is_empty(),
        verification_status: r.verification.as_ref().map(|v| v.status),
    }
}
```

In `artifacts.rs`, change `pub(crate) fn sha256_file` to `pub fn sha256_file`, and add this to `impl ArtifactStore`:

```rust
    /// Like `blob_path`, but only for a well-formed sha256 (64 lowercase hex).
    /// Use this for any caller-supplied digest (e.g. an HTTP path segment).
    pub fn blob_path_checked(&self, sha256: &str) -> Option<PathBuf> {
        let ok = sha256.len() == 64 && sha256.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
        ok.then(|| self.blob_path(sha256))
    }
```

In `report/mod.rs`, add `pub mod summary;` and `pub use summary::{completeness, summarize, Completeness, ReportSummary};`, and add `sha256_file` to the `pub use artifacts::{…}` line.

- [ ] **Step 4: Run the tests and check they pass**

Run: `cargo test -p rupu-coverage && cargo clippy -p rupu-coverage --all-targets -- -D warnings`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-coverage/src/report
git commit -m "feat(coverage): report summary + completeness; checked artifact blob path

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: Slim list rows + `GET /api/findings/:id` (rupu-cp)

**Files:**
- Modify: `crates/rupu-cp/src/api/findings.rs`
- Regenerate: `apps/rupu-macos/Fixtures/*.json` via `make macos-fixtures` (commit any drift)

**Interfaces:**
- Consumes: `rupu_coverage::report::{summarize, ReportSummary, sha256_file}`; `crate::api::source::resolve_under_workspace`; `crate::api::code::load_workspace`.
- Produces:
  - `FindingOut.report_summary: Option<ReportSummary>` (`skip_serializing_if = "Option::is_none"`). List rows always have `record.report = None`.
  - `GET /api/findings/:id` → `FindingDetail { #[serde(flatten)] finding: FindingOut, evidence_status: Vec<ClaimState> }`, where `finding.record.report` is populated. It returns 404 `{"error": "finding <id> not found"}` on a miss.
  - `#[serde(rename_all = "lowercase")] pub enum ClaimState { Current, Changed, Missing, Unknown }`, one per `report.evidence[i]`, empty for summary findings.

- [ ] **Step 1: Write the failing tests** in `findings.rs`'s test module. Reuse the on-disk setup of the existing test `collect_all_findings_computes_permalink_from_workspace_remote`: it writes `workspaces/ws1.toml` for a workspace at a temp repo path and writes `paths.findings` as JSONL. Add a helper that writes one full-profile record, then:

```rust
    fn full_record(id: &str) -> FindingRecord {
        let report: rupu_coverage::FindingReport = serde_json::from_str(include_str!(
            "../../../rupu-coverage/tests/fixtures/finding_report/valid_full.json"
        ))
        .unwrap();
        let mut rec = finding(id, Severity::Critical, "2026-09-29T00:00:00Z").record;
        rec.profile = rupu_coverage::FindingProfile::Full;
        rec.summary = report.title.clone();
        rec.report = Some(report);
        rec
    }

    #[test]
    fn list_rows_carry_report_summary_not_report() {
        let mut out = finding("f1", Severity::Critical, "2026-09-29T00:00:00Z");
        out.record = full_record("f1");
        let row = out.into_list_row();
        assert!(row.record.report.is_none());
        let s = row.report_summary.expect("summary for full-profile rows");
        assert_eq!(s.completeness.total, 11);
        let json = serde_json::to_value(&row).unwrap();
        assert!(json.get("report").is_none());
        assert!(json["report_summary"]["root_cause"].is_string());
    }

    #[test]
    fn summary_rows_have_no_report_summary_key() {
        let row = finding("f2", Severity::High, "2026-09-29T00:00:00Z").into_list_row();
        let json = serde_json::to_value(&row).unwrap();
        assert!(json.get("report_summary").is_none());
    }

    #[test]
    fn claim_states_compare_file_hashes() {
        let ws = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(ws.path().join("src/routes")).unwrap();
        std::fs::write(ws.path().join("src/routes/notes.rs"), "fn a() {}\n").unwrap();
        let mut report = full_record("f3").report.unwrap();
        let current = rupu_coverage::report::sha256_file(&ws.path().join("src/routes/notes.rs")).unwrap();
        report.evidence[0].sha256 = Some(current.clone());
        let mut changed = report.evidence[0].clone();
        changed.sha256 = Some("0".repeat(64));
        let mut missing = report.evidence[0].clone();
        missing.file = Some("src/gone.rs".into());
        let mut unknown = report.evidence[0].clone();
        unknown.sha256 = None;
        report.evidence = vec![report.evidence[0].clone(), changed, missing, unknown];
        assert_eq!(
            claim_states(ws.path(), &report),
            vec![ClaimState::Current, ClaimState::Changed, ClaimState::Missing, ClaimState::Unknown]
        );
    }
```

Then add an HTTP test (copy the pattern of an existing handler test that builds `AppState::new(dir, PricingConfig::default())` and uses `routes().with_state(state)` with `tower::ServiceExt::oneshot`). It writes one full-profile finding with id `fnd_detail` into a workspace, requests `GET /api/findings/fnd_detail` and asserts 200, `json["report"]["title"]` set and `json["evidence_status"]` an array of length `report.evidence.len()`. Also request `GET /api/findings/fnd_nope` and assert 404.

- [ ] **Step 2: Run the tests and check they fail**

Run: `cargo test -p rupu-cp api::findings`
Expected: compile errors (`into_list_row`, `report_summary`, `claim_states` and `ClaimState` don't exist yet).

- [ ] **Step 3: Implement**

In `findings.rs`:

```rust
use rupu_coverage::report::{summarize, ReportSummary};

// in FindingOut, after `permalink`:
    /// Present for full-profile findings in LIST responses: the fields a row
    /// needs, without the report body (which `GET /api/findings/:id` serves).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub report_summary: Option<ReportSummary>,
```

Initialize `report_summary: None` wherever `FindingOut { .. }` is constructed (`collect_all_findings`, test helpers). Then:

```rust
impl FindingOut {
    /// The list-endpoint shape: summary fields in, report body out.
    pub(crate) fn into_list_row(mut self) -> Self {
        self.report_summary = self.record.report.as_ref().map(summarize);
        self.record.report = None;
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ClaimState {
    /// The file still hashes to the value recorded with the claim.
    Current,
    /// The file exists but its contents changed since the finding was recorded.
    Changed,
    /// The file is gone (or escapes the workspace).
    Missing,
    /// No file or no recorded hash to compare (binary targets, summary claims).
    Unknown,
}

pub(crate) fn claim_states(workspace: &std::path::Path, r: &rupu_coverage::FindingReport) -> Vec<ClaimState> {
    r.evidence
        .iter()
        .map(|c| match (&c.file, &c.sha256) {
            (Some(file), Some(recorded)) => match crate::api::source::resolve_under_workspace(workspace, file) {
                Ok(p) if p.is_file() => match rupu_coverage::report::sha256_file(&p) {
                    Ok(now) if &now == recorded => ClaimState::Current,
                    Ok(_) => ClaimState::Changed,
                    Err(_) => ClaimState::Missing,
                },
                _ => ClaimState::Missing,
            },
            _ => ClaimState::Unknown,
        })
        .collect()
}

#[derive(Debug, Serialize)]
pub struct FindingDetail {
    #[serde(flatten)]
    pub finding: FindingOut,
    pub evidence_status: Vec<ClaimState>,
}

async fn get_finding(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<FindingDetail>> {
    let global = s.global_dir.clone();
    let found = tokio::task::spawn_blocking(move || {
        collect_all_findings(&global).into_iter().find(|f| f.record.id == id).ok_or(id)
    })
    .await
    .map_err(|e| ApiError::internal(e.to_string()))?;
    let mut finding = found.map_err(|id| ApiError::not_found(format!("finding {id} not found")))?;
    if let Ok(run) = s.run_store.load(&finding.record.declared_by.run_id) {
        finding.workflow_name = Some(run.workflow_name.clone());
    }
    let evidence_status = match (&finding.record.report, crate::api::code::load_workspace(&s, &finding.ws_id)) {
        (Some(report), Ok(ws)) => claim_states(std::path::Path::new(&ws.path), report),
        (Some(report), Err(_)) => vec![ClaimState::Unknown; report.evidence.len()],
        (None, _) => Vec::new(),
    };
    Ok(Json(FindingDetail { finding, evidence_status }))
}
```

Adapt to the real names in this file:
- how `list_findings` joins `workflow_name` from `s.run_store.load` (match its field access exactly);
- `ApiError` constructors (`not_found`, `internal`);
- the `load_workspace` return type (it returns a `Workspace`; use its path field as `list_findings`/`code.rs` do).

In `list_findings`, map every row through `.into_list_row()` before `build_response`. Register the route:

```rust
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/findings", get(list_findings))
        .route("/api/findings/:id", get(get_finding))
}
```

- [ ] **Step 4: Run the tests and check they pass; regenerate fixtures**

Run: `cargo test -p rupu-cp api::findings`, then `make macos-fixtures` (from the repo root), then `cargo test -p rupu-cp` and `cargo clippy -p rupu-cp --all-targets -- -D warnings`.
Expected: PASS. Fixture drift is only expected if a fixture finding is full-profile; commit whatever `make macos-fixtures` changes.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-cp apps/rupu-macos/Fixtures
git commit -m "feat(cp): slim findings list rows + GET /api/findings/:id with claim staleness

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: `GET /api/findings/:id/artifacts/:sha256` (rupu-cp)

**Files:**
- Modify: `crates/rupu-cp/src/api/findings.rs`, `crates/rupu-cp/Cargo.toml` (move `tokio-util` to `[dependencies]` with feature `io` if it is only a dev-dependency; the version comes from the root `Cargo.toml`)

**Interfaces:**
- Consumes: `rupu_coverage::report::{ArtifactStore, ArtifactStorage, ArtifactKind, sha256_file}`; `get_finding`'s lookup (factor it into `fn find_finding(global: &Path, id: &str) -> Option<FindingOut>`).
- Produces: a route returning the artifact bytes (these rules are the contract):
  1. 400 unless `sha256` is 64 lowercase hex.
  2. 404 unless the finding exists **and** its `report.artifacts` contains an entry with that `sha256`. This stops the endpoint from serving arbitrary store blobs.
  3. `stored: copied` → stream `<global_dir>/findings/artifacts/<aa>/<sha256>`, or 404 `{"error":"artifact blob missing from the store"}` if it is absent.
  4. `stored: external` with `host: Some(h)` → 404 `{"error":"artifact is stored on host <h>; remote fetch is not supported yet"}`.
  5. `stored: external`, no host → resolve `artifact.path` under the finding's workspace (`resolve_under_workspace`). If the file is missing → 404 `"artifact is no longer in the workspace"`. If it hashes differently → 409 `"artifact changed since the finding was recorded"`. Otherwise stream it.
  6. Headers: `kind == Some(Text)` → `Content-Type: text/plain; charset=utf-8` and `Content-Disposition: inline; filename="<basename>"`. Anything else → `Content-Type: application/octet-stream` and `Content-Disposition: attachment; filename="<basename>"`. Always `X-Content-Type-Options: nosniff`. Build `basename` from the last path segment of `artifact.path`, replacing `"` and control characters with `_`.

- [ ] **Step 1: Write the failing tests** (HTTP tests, same harness as Task 2):
  - A copied text artifact returns 200 with body bytes, `content-type` starting with `text/plain`, `x-content-type-options: nosniff`, and a disposition starting with `inline`.
  - A copied binary artifact returns `application/octet-stream` with an `attachment` disposition.
  - A sha that is not listed in the finding's artifacts, but whose blob does exist in the store, returns 404. Write the blob file directly into `<global>/findings/artifacts/<aa>/<sha>`.
  - Uppercase or short sha returns 400.
  - External with a host returns 404 with a body mentioning the host.
  - External and local, with the workspace file changed after recording, returns 409.
  - A filename containing `"` is sanitized in the header.

  Seed copied artifacts by computing the sha with `rupu_coverage::report::sha256_file` over a temp file and copying it to the blob path. Build each finding's `report.artifacts` by hand as `ArtifactRef { path, sha256, size, kind, stored, host }`.

- [ ] **Step 2: Run and check they fail**

Run: `cargo test -p rupu-cp api::findings`
Expected: FAIL (route missing, 404 for everything).

- [ ] **Step 3: Implement**

```rust
use axum::{body::Body, http::{header, HeaderValue, StatusCode}, response::Response};
use rupu_coverage::report::{ArtifactKind, ArtifactStorage, ArtifactStore};

fn safe_filename(path: &str) -> String {
    let base = path.rsplit('/').next().unwrap_or(path);
    let cleaned: String = base.chars().map(|c| if c == '"' || c.is_control() { '_' } else { c }).collect();
    if cleaned.is_empty() { "artifact".to_string() } else { cleaned }
}

async fn get_artifact(
    State(s): State<AppState>,
    Path((id, sha)): Path<(String, String)>,
) -> Result<Response, ApiError> {
    let store = ArtifactStore::new(s.global_dir.join("findings").join("artifacts"));
    let blob = store.blob_path_checked(&sha).ok_or_else(|| ApiError::bad_request("artifact id must be a 64-character lowercase sha256"))?;
    let global = s.global_dir.clone();
    let id_for_lookup = id.clone();
    let finding = tokio::task::spawn_blocking(move || find_finding(&global, &id_for_lookup))
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?
        .ok_or_else(|| ApiError::not_found(format!("finding {id} not found")))?;
    let artifact = finding
        .record
        .report
        .as_ref()
        .and_then(|r| r.artifacts.iter().find(|a| a.sha256 == sha))
        .cloned()
        .ok_or_else(|| ApiError::not_found("this finding does not reference that artifact"))?;

    let path = match (artifact.stored, artifact.host.as_deref()) {
        (Some(ArtifactStorage::External), Some(host)) => {
            return Err(ApiError::not_found(format!(
                "artifact is stored on host {host}; remote fetch is not supported yet"
            )))
        }
        (Some(ArtifactStorage::External), None) => {
            let ws = crate::api::code::load_workspace(&s, &finding.ws_id)?;
            let p = crate::api::source::resolve_under_workspace(std::path::Path::new(&ws.path), &artifact.path)
                .map_err(|_| ApiError::not_found("artifact is no longer in the workspace"))?;
            if !p.is_file() {
                return Err(ApiError::not_found("artifact is no longer in the workspace"));
            }
            let expected = sha.clone();
            let p2 = p.clone();
            let now = tokio::task::spawn_blocking(move || rupu_coverage::report::sha256_file(&p2))
                .await
                .map_err(|e| ApiError::internal(e.to_string()))?
                .map_err(|e| ApiError::internal(e.to_string()))?;
            if now != expected {
                return Err(ApiError(StatusCode::CONFLICT, "artifact changed since the finding was recorded".into()));
            }
            p
        }
        _ => {
            if !blob.is_file() {
                return Err(ApiError::not_found("artifact blob missing from the store"));
            }
            blob
        }
    };

    let file = tokio::fs::File::open(&path).await.map_err(|e| ApiError::internal(e.to_string()))?;
    let body = Body::from_stream(tokio_util::io::ReaderStream::new(file));
    let name = safe_filename(&artifact.path);
    let (ctype, disposition) = match artifact.kind {
        Some(ArtifactKind::Text) => ("text/plain; charset=utf-8", format!("inline; filename=\"{name}\"")),
        _ => ("application/octet-stream", format!("attachment; filename=\"{name}\"")),
    };
    let mut resp = Response::new(body);
    let h = resp.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static(ctype));
    h.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    h.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&disposition).unwrap_or_else(|_| HeaderValue::from_static("attachment")),
    );
    Ok(resp)
}
```

Use the real `ApiError` shape for 409. If `ApiError` is a tuple struct `ApiError(StatusCode, String)` as the survey says, construct it directly as above; otherwise add a `conflict` constructor. Refactor `get_finding` to use `find_finding`. Add `.route("/api/findings/:id/artifacts/:sha256", get(get_artifact))`.

- [ ] **Step 4: Run and check they pass**

Run: `cargo test -p rupu-cp && cargo clippy -p rupu-cp --all-targets -- -D warnings`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-cp Cargo.lock
git commit -m "feat(cp): serve a finding's referenced artifacts (text inline, binary attachment, never HTML)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: Web report model, API helpers, shared utilities

**Files:**
- Create: `crates/rupu-cp/web/src/lib/findingReport.ts`, `crates/rupu-cp/web/src/lib/findingReport.test.ts`
- Modify: `crates/rupu-cp/web/src/lib/api.ts` (types + `getFinding` + `findingArtifactUrl`)

**Interfaces:**
- Produces (all exported from `lib/findingReport.ts`):
  - the report types listed below (`FindingReport`, `OrSentinel<T>`, `ChainHop`, `EvidenceClaim`, `ArtifactRef`, `ReportSummary`, `Completeness`, `ClaimState`, …)
  - `isSentinel`
  - `isGapSentinel`
  - `sentinelLabel`
  - `codeHref(wsId, path, line?)`
  - `copyText(text): Promise<boolean>`
  - `formatBytes(n)`
- Adds to `lib/api.ts`:
  - fields on `FindingRecord`: `profile?: 'full' | 'summary'`, `report_summary?: ReportSummary | null`, `report?: FindingReport | null`, `target_ref?: string | null`
  - `interface FindingDetail extends FindingOut { evidence_status: ClaimState[] }`
  - `api.getFinding(id): Promise<FindingDetail>`
  - `findingArtifactUrl(id, sha256): string`

- [ ] **Step 1: Write the failing tests** — `lib/findingReport.test.ts`:

```ts
import { describe, it, expect, vi, afterEach } from 'vitest';
import { isSentinel, isGapSentinel, sentinelLabel, codeHref, copyText, formatBytes } from './findingReport';
import { api, findingArtifactUrl } from './api';

afterEach(() => vi.restoreAllMocks());

describe('findingReport helpers', () => {
  it('classifies sentinels', () => {
    expect(isSentinel('None')).toBe(true);
    expect(isSentinel({ diff: 'x' })).toBe(false);
    expect(isGapSentinel('Unknown')).toBe(true);
    expect(isGapSentinel('Not Provided — needs hardware')).toBe(true);
    expect(isGapSentinel('None Provided')).toBe(false);
    expect(sentinelLabel('Not Provided — needs hardware')).toBe('Not provided: needs hardware');
    expect(sentinelLabel('Unknown')).toBe('Unknown');
  });

  it('builds code links', () => {
    expect(codeHref('ws 1', 'src/a b.rs', 12)).toBe('/projects/ws%201/code?path=src%2Fa%20b.rs&line=12');
    expect(codeHref('ws', 'x.rs')).toBe('/projects/ws/code?path=x.rs');
  });

  it('formats bytes', () => {
    expect(formatBytes(0)).toBe('0 B');
    expect(formatBytes(1536)).toBe('1.5 KB');
    expect(formatBytes(5 * 1024 * 1024)).toBe('5.0 MB');
  });

  it('copyText reports failure instead of throwing', async () => {
    Object.assign(navigator, { clipboard: { writeText: vi.fn().mockRejectedValue(new Error('denied')) } });
    await expect(copyText('x')).resolves.toBe(false);
    Object.assign(navigator, { clipboard: { writeText: vi.fn().mockResolvedValue(undefined) } });
    await expect(copyText('x')).resolves.toBe(true);
  });

  it('getFinding and artifact URLs encode ids', async () => {
    const fetchMock = vi.spyOn(globalThis, 'fetch').mockResolvedValue(new Response(JSON.stringify({ id: 'fnd_1' })));
    await api.getFinding('fnd_1');
    expect(fetchMock.mock.calls[0][0]).toBe('/api/findings/fnd_1');
    expect(findingArtifactUrl('fnd/1', 'ab')).toBe('/api/findings/fnd%2F1/artifacts/ab');
  });
});
```

(Add `// @vitest-environment jsdom` at the top, because `navigator` is needed.)

- [ ] **Step 2: Run and check it fails**

Run: `cd crates/rupu-cp/web && npx vitest run src/lib/findingReport.test.ts`
Expected: FAIL (the module doesn't exist).

- [ ] **Step 3: Implement** `lib/findingReport.ts`:

```ts
// Typed model of the structured finding report (mirrors
// rupu-coverage/src/report/types.rs and summary.rs) plus small helpers the
// report views share.

export type RiskLevel = 'Low' | 'Medium' | 'High' | 'Critical';
export type Likelihood = 'Low' | 'Medium' | 'High';
/** A field that is either real content or a sentinel string. */
export type OrSentinel<T> = T | string;

export interface Ownership { owner: string; product: string; affected_component: string; source_repository: string }
export interface Ticket { type: string; identifier: string; url?: string; notes?: string }
export interface Rating { impact: RiskLevel; likelihood: Likelihood; risk_rating: RiskLevel; risk_factor: RiskLevel; cvss_v3: string }
export interface ChainHop {
  label: string; file?: string; lines?: [number, number]; binary_va?: string;
  gate?: string; passes_because?: string; role: 'source' | 'hop' | 'sink';
}
export interface EvidenceClaim {
  claim: string; file?: string; lines?: [number, number]; binary_va?: string;
  excerpt?: string; lang?: string; sha256?: string; artifact?: string;
}
export interface Patch { diff: string; notes?: string }
export interface CiDetection { stage: string; body: string; command?: string; expect: string }
export interface RegressionTest { body: string; command: string; expect_vulnerable: string; expect_patched: string }
export interface CrossRef { finding_id: string; relation: 'duplicate' | 'sibling' | 'prerequisite' | 'supersedes'; note?: string }
export interface ArtifactRef {
  path: string; sha256: string; size: number; kind?: 'text' | 'binary';
  stored?: 'copied' | 'external'; host?: string;
}
export type VerificationStatus = 'unverified' | 'confirmed' | 'disputed' | 'inconclusive';
export interface Verification { status: VerificationStatus; by_run?: string; notes?: string }

export interface FindingReport {
  title: string;
  ownership: Ownership;
  tickets: OrSentinel<Ticket[]>;
  rating: Rating;
  category: string;
  attack_vector: string;
  cwe?: string[];
  description: string;
  impact: string;
  location: { input: string; output: string };
  root_cause: string;
  call_chain: OrSentinel<ChainHop[]>;
  evidence: EvidenceClaim[];
  remediation: string;
  recommended_patch: OrSentinel<Patch>;
  ci_cd_detection: OrSentinel<CiDetection>;
  regression_test: OrSentinel<RegressionTest>;
  replication_steps: string[];
  cross_references: OrSentinel<CrossRef[]>;
  references: string;
  artifacts?: ArtifactRef[];
  verification?: Verification;
}

export interface Completeness { filled: number; total: number; gaps: string[] }
export interface ReportSummary {
  owner: string; product: string; cwe: string[]; root_cause: string; chain: string[];
  completeness: Completeness; has_poc: boolean; verification_status?: VerificationStatus | null;
}
export type ClaimState = 'current' | 'changed' | 'missing' | 'unknown';

export function isSentinel<T>(v: OrSentinel<T>): v is string {
  return typeof v === 'string';
}

const NOT_PROVIDED = 'Not Provided — ';

/** `Unknown` and `Not Provided — …` are gaps; `None`, `None Provided`,
 *  `Not Applicable` are real answers. */
export function isGapSentinel(s: string): boolean {
  return s.trim() === 'Unknown' || s.startsWith(NOT_PROVIDED);
}

export function sentinelLabel(s: string): string {
  return s.startsWith(NOT_PROVIDED) ? `Not provided: ${s.slice(NOT_PROVIDED.length)}` : s;
}

/** Deep link into a project's Code tab. */
export function codeHref(wsId: string, path: string, line?: number): string {
  const base = `/projects/${encodeURIComponent(wsId)}/code?path=${encodeURIComponent(path)}`;
  return line !== undefined ? `${base}&line=${line}` : base;
}

export async function copyText(text: string): Promise<boolean> {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    return false;
  }
}

export function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  if (n < 1024 * 1024 * 1024) return `${(n / (1024 * 1024)).toFixed(1)} MB`;
  return `${(n / (1024 * 1024 * 1024)).toFixed(1)} GB`;
}
```

In `lib/api.ts`:
- import the types from `./findingReport`.
- add to `FindingRecord`: `target_ref?: string | null; profile?: 'full' | 'summary'; report_summary?: ReportSummary | null; report?: FindingReport | null;`
- add `export interface FindingDetail extends FindingOut { evidence_status: ClaimState[] }`
- add to the `api` object, next to `getFindings`: `getFinding(id: string): Promise<FindingDetail> { return request<FindingDetail>(`/api/findings/${encodeURIComponent(id)}`); },`
- add the exported function `export function findingArtifactUrl(id: string, sha256: string): string { return `/api/findings/${encodeURIComponent(id)}/artifacts/${encodeURIComponent(sha256)}`; }`

- [ ] **Step 4: Run and check it passes**

Run: `cd crates/rupu-cp/web && npx vitest run src/lib/findingReport.test.ts && npx tsc --noEmit -p .`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-cp/web/src/lib
git commit -m "feat(cp-web): typed finding report model, getFinding, artifact URLs, shared helpers

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 5: Report section components

**Files:** Create each of these under `crates/rupu-cp/web/src/components/findings/report/`:
- `Section.tsx`
- `ReportHeader.tsx`
- `CallChain.tsx`
- `EvidenceClaims.tsx`
- `CommandBlock.tsx`
- `FixSections.tsx` (patch, CI/CD detection, regression test)
- `ReplicationSteps.tsx`
- `ArtifactBrowser.tsx`
- `CrossReferences.tsx`
- `report.test.tsx`

**Interfaces:**
- Consumes: Task 4 types/helpers; `components/transcript/Markdown` (default, `{text}`); `components/transcript/DiffView` (default, `{diff, path?, editKind?}`); `components/CodeHighlight` (default, `{code, language?}`, with `HIGHLIGHTABLE_LANGUAGES`); `components/coverage/SeverityChip` (`{severity}`); `lib/api` (`normFindingSeverity`, `findingArtifactUrl`).
- Produces the default exports below. Each takes typed props and never throws on a sentinel.
  - `Section({ id, title, hint?, children })`
  - `ReportHeader({ finding, report })`
  - `CallChain({ chain, wsId })`
  - `EvidenceClaims({ claims, states, wsId })`
  - `CommandBlock({ label, command })`
  - `FixSections({ report })`
  - `ReplicationSteps({ steps })`
  - `ArtifactBrowser({ findingId, artifacts })`
  - `CrossReferences({ refs, references })`

- [ ] **Step 1: Write the failing tests** — `report.test.tsx`. Load the fixture with `import fixture from '../../../../../../rupu-coverage/tests/fixtures/finding_report/valid_full.json'`. If the relative JSON import is not allowed by the TS/vite config, copy the fixture into `src/components/findings/report/__fixtures__/valid_full.json` and import that; `tsconfig` must have `resolveJsonModule`. Assert:
- `CallChain` renders three hops in source→sink order, labels visible, the sink marked with `aria-label="sink"`. Each hop with a file renders a link whose `href` equals `codeHref(wsId, file, lines[0])`. A sentinel chain (`'Not Provided — n/a'`) renders the text `Not provided: n/a` and no list.
- `EvidenceClaims` with states `['changed']` shows a "Code changed since recorded" badge. With `['current']` it shows no badge. The excerpt renders in a `<pre>`.
- `FixSections`:
  - `recommended_patch` as an object renders a diff (the text `find_by_id_for_owner` is visible).
  - A `regression_test` sentinel renders its label.
  - The regression block shows both "Vulnerable build" and "Patched build" texts.
- `CommandBlock` has a "Copy" button. Clicking it calls `navigator.clipboard.writeText` with the command, and the button shows "Copied".
- `ArtifactBrowser`:
  - A text artifact: clicking it fetches `findingArtifactUrl(id, sha)` (mock `fetch`) and shows the text in a `<pre>`.
  - A binary artifact renders a download link with that URL and `download` attribute.
  - An external artifact with `host` shows "on host".
- `ReportHeader` renders the title, a severity chip derived from `rating.risk_rating`, the owner cell styled as a gap (`data-gap="true"`) when `Unknown`, and each CWE as a link to `https://cwe.mitre.org/data/definitions/<n>.html`.
- `CrossReferences` with `'None'` renders "None". With a list it renders links to `/findings/<id>`.

- [ ] **Step 2: Run and check it fails**

Run: `cd crates/rupu-cp/web && npx vitest run src/components/findings/report`
Expected: FAIL (the modules don't exist).

- [ ] **Step 3: Implement.** Keep each file focused; use this code.

`Section.tsx`:
```tsx
import type { ReactNode } from 'react';

export default function Section({ id, title, hint, children }: { id: string; title: string; hint?: string; children: ReactNode }) {
  return (
    <section id={id} className="scroll-mt-4 space-y-2">
      <h2 className="flex items-baseline gap-2 text-meta font-semibold uppercase tracking-wide text-ink-mute">
        {title}
        {hint && <span className="font-mono normal-case tracking-normal text-note font-normal">{hint}</span>}
      </h2>
      {children}
    </section>
  );
}
```

`CommandBlock.tsx`:
```tsx
import { useState } from 'react';
import { copyText } from '../../../lib/findingReport';

export default function CommandBlock({ label, command }: { label: string; command: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <div className="overflow-hidden rounded-md border border-border bg-panel">
      <div className="flex items-center justify-between border-b border-border px-3 py-1">
        <span className="text-note text-ink-mute">{label}</span>
        <button
          type="button"
          onClick={() => void copyText(command).then((ok) => { setCopied(ok); if (ok) setTimeout(() => setCopied(false), 1500); })}
          className="rounded border border-border px-2 py-0.5 text-note text-ink-dim hover:bg-surface"
        >
          {copied ? 'Copied' : 'Copy'}
        </button>
      </div>
      <pre className="overflow-x-auto px-3 py-2 text-note font-mono text-ink leading-snug whitespace-pre">{command}</pre>
    </div>
  );
}
```

`CallChain.tsx`:
```tsx
import { Link } from 'react-router-dom';
import { codeHref, isSentinel, sentinelLabel, type ChainHop, type OrSentinel } from '../../../lib/findingReport';

const DOT: Record<ChainHop['role'], string> = {
  source: 'bg-ink-mute',
  hop: 'bg-brand-500',
  sink: 'bg-sev-critical ring-4 ring-sev-critical/20',
};

export default function CallChain({ chain, wsId }: { chain: OrSentinel<ChainHop[]>; wsId?: string }) {
  if (isSentinel(chain)) return <p className="text-ui text-ink-mute">{sentinelLabel(chain)}</p>;
  return (
    <ol className="space-y-0">
      {chain.map((h, i) => (
        <li key={`${h.label}-${i}`} className="grid grid-cols-[1.25rem_minmax(0,1fr)] gap-3">
          <div className="flex flex-col items-center">
            <span aria-label={h.role} className={`mt-1.5 h-2.5 w-2.5 rounded-full ${DOT[h.role]}`} />
            {i < chain.length - 1 && <span className="w-px flex-1 bg-border" />}
          </div>
          <div className="min-w-0 space-y-0.5 pb-3">
            <div className={`font-mono text-ui ${h.role === 'sink' ? 'text-sev-critical' : 'text-ink'}`}>{h.label}</div>
            {h.file &&
              (wsId ? (
                <Link to={codeHref(wsId, h.file, h.lines?.[0])} className="font-mono text-note text-brand-700 hover:underline">
                  {h.file}{h.lines ? `:${h.lines[0]}-${h.lines[1]}` : ''}
                </Link>
              ) : (
                <span className="font-mono text-note text-ink-mute">{h.file}{h.lines ? `:${h.lines[0]}-${h.lines[1]}` : ''}</span>
              ))}
            {h.binary_va && <span className="font-mono text-note text-ink-mute">{h.binary_va}</span>}
            {h.gate && (
              <p className="text-ui text-ink-dim">
                Gate: {h.gate}
                {h.passes_because && <> — <span className="text-ok">passes</span>: {h.passes_because}</>}
              </p>
            )}
          </div>
        </li>
      ))}
    </ol>
  );
}
```

`EvidenceClaims.tsx`:
```tsx
import { Link } from 'react-router-dom';
import CodeHighlight, { HIGHLIGHTABLE_LANGUAGES, type Language } from '../../CodeHighlight';
import { codeHref, type ClaimState, type EvidenceClaim } from '../../../lib/findingReport';

const BADGE: Partial<Record<ClaimState, { text: string; cls: string }>> = {
  changed: { text: 'Code changed since recorded', cls: 'bg-warn-bg text-warn' },
  missing: { text: 'File no longer present', cls: 'bg-err-bg text-err' },
};

export default function EvidenceClaims({ claims, states, wsId }: { claims: EvidenceClaim[]; states: ClaimState[]; wsId?: string }) {
  return (
    <div className="space-y-2">
      {claims.map((c, i) => {
        const badge = BADGE[states[i] ?? 'unknown'];
        const loc = c.file ? `${c.file}${c.lines ? `:${c.lines[0]}-${c.lines[1]}` : ''}` : c.binary_va;
        return (
          <div key={i} className="overflow-hidden rounded-md border border-border bg-panel">
            <div className="flex flex-wrap items-baseline gap-2 border-b border-border px-3 py-1.5">
              {loc &&
                (c.file && wsId ? (
                  <Link to={codeHref(wsId, c.file, c.lines?.[0])} className="font-mono text-note text-brand-700 hover:underline">{loc}</Link>
                ) : (
                  <span className="font-mono text-note text-ink-mute">{loc}</span>
                ))}
              <span className="min-w-0 flex-1 text-ui text-ink-dim">{c.claim}</span>
              {badge && <span className={`rounded px-1.5 py-0.5 text-note ${badge.cls}`}>{badge.text}</span>}
              {c.artifact && <span className="rounded bg-surface px-1.5 py-0.5 text-note text-ink-dim ring-1 ring-border">artifact: {c.artifact}</span>}
            </div>
            {c.excerpt &&
              (c.lang && HIGHLIGHTABLE_LANGUAGES.has(c.lang) ? (
                <CodeHighlight code={c.excerpt} language={c.lang as Language} />
              ) : (
                <pre className="overflow-x-auto px-3 py-2 text-note font-mono text-ink leading-snug whitespace-pre">{c.excerpt}</pre>
              ))}
          </div>
        );
      })}
    </div>
  );
}
```
(If `CodeHighlight` doesn't render a `<pre>`, make the test look for the highlighted text instead of a `<pre>`. Read `CodeHighlight.tsx` first.)

`FixSections.tsx`:
```tsx
import DiffView from '../../transcript/DiffView';
import Markdown from '../../transcript/Markdown';
import CommandBlock from './CommandBlock';
import Section from './Section';
import { isSentinel, sentinelLabel, type FindingReport } from '../../../lib/findingReport';

function SentinelNote({ value }: { value: string }) {
  return <p className="text-ui text-ink-mute">{sentinelLabel(value)}</p>;
}

export default function FixSections({ report }: { report: FindingReport }) {
  const patch = report.recommended_patch;
  const ci = report.ci_cd_detection;
  const reg = report.regression_test;
  return (
    <>
      <Section id="s-patch" title="Recommended patch">
        {isSentinel(patch) ? <SentinelNote value={patch} /> : (
          <div className="space-y-2">
            <div className="overflow-hidden rounded-md border border-border"><DiffView diff={patch.diff} /></div>
            {patch.notes && <div className="text-ink-dim"><Markdown text={patch.notes} /></div>}
          </div>
        )}
      </Section>
      <Section id="s-ci" title="CI/CD detection" hint={isSentinel(ci) ? undefined : `stage: ${ci.stage}`}>
        {isSentinel(ci) ? <SentinelNote value={ci} /> : (
          <div className="space-y-2">
            <div className="text-ink-dim"><Markdown text={ci.body} /></div>
            {ci.command && <CommandBlock label="Command" command={ci.command} />}
            <p className="text-ui text-ink-dim"><span className="font-semibold text-ink">Fails when:</span> {ci.expect}</p>
          </div>
        )}
      </Section>
      <Section id="s-reg" title="Regression test">
        {isSentinel(reg) ? <SentinelNote value={reg} /> : (
          <div className="space-y-2">
            <div className="text-ink-dim"><Markdown text={reg.body} /></div>
            <CommandBlock label="Command" command={reg.command} />
            <div className="grid gap-2 sm:grid-cols-2">
              <div className="rounded-md bg-err-bg px-3 py-2 text-ui text-ink-dim">
                <div className="text-meta font-semibold uppercase tracking-wide text-err">Vulnerable build</div>{reg.expect_vulnerable}
              </div>
              <div className="rounded-md bg-ok-bg px-3 py-2 text-ui text-ink-dim">
                <div className="text-meta font-semibold uppercase tracking-wide text-ok">Patched build</div>{reg.expect_patched}
              </div>
            </div>
          </div>
        )}
      </Section>
    </>
  );
}
```

`ReplicationSteps.tsx`:
```tsx
import Markdown from '../../transcript/Markdown';

export default function ReplicationSteps({ steps }: { steps: string[] }) {
  return (
    <ol className="space-y-2">
      {steps.map((s, i) => (
        <li key={i} className="grid grid-cols-[1.5rem_minmax(0,1fr)] gap-2">
          <span className="grid h-5 w-5 place-items-center rounded-full bg-surface text-meta font-semibold text-ink ring-1 ring-border">{i + 1}</span>
          <div className="text-ink-dim [&_p]:text-ui"><Markdown text={s} /></div>
        </li>
      ))}
    </ol>
  );
}
```

`ArtifactBrowser.tsx`:
```tsx
import { useState } from 'react';
import { findingArtifactUrl } from '../../../lib/api';
import { formatBytes, type ArtifactRef } from '../../../lib/findingReport';

const PREVIEW_LIMIT = 256 * 1024;

export default function ArtifactBrowser({ findingId, artifacts }: { findingId: string; artifacts: ArtifactRef[] }) {
  const [selected, setSelected] = useState<ArtifactRef | null>(null);
  const [text, setText] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  async function open(a: ArtifactRef) {
    setSelected(a);
    setText(null);
    setError(null);
    if (a.kind !== 'text' || a.host) return;
    if (a.size > PREVIEW_LIMIT) { setError(`Too large to preview (${formatBytes(a.size)}); download it instead.`); return; }
    try {
      const res = await fetch(findingArtifactUrl(findingId, a.sha256), { credentials: 'same-origin' });
      if (!res.ok) { setError(await res.text()); return; }
      setText(await res.text());
    } catch (e) {
      setError(String(e));
    }
  }

  return (
    <div className="grid overflow-hidden rounded-md border border-border bg-panel md:grid-cols-[minmax(12rem,16rem)_minmax(0,1fr)]">
      <ul className="border-b border-border p-1.5 md:border-b-0 md:border-r">
        {artifacts.map((a) => (
          <li key={`${a.path}-${a.sha256}`}>
            <button
              type="button"
              onClick={() => void open(a)}
              className={`flex w-full justify-between gap-2 rounded px-2 py-1 text-left font-mono text-note ${selected?.sha256 === a.sha256 ? 'bg-brand-50 text-ink' : 'text-ink-dim hover:bg-surface'}`}
            >
              <span className="truncate">{a.path}</span>
              <span className="shrink-0 text-ink-mute">{formatBytes(a.size)}</span>
            </button>
          </li>
        ))}
      </ul>
      <div className="min-w-0">
        {!selected && <p className="px-3 py-2 text-ui text-ink-mute">Select an artifact to preview it.</p>}
        {selected && (
          <>
            <div className="flex flex-wrap items-center justify-between gap-2 border-b border-border px-3 py-1.5 font-mono text-note text-ink-mute">
              <span className="truncate">{selected.path}</span>
              {selected.host ? (
                <span>stored on host {selected.host}</span>
              ) : (
                <a href={findingArtifactUrl(findingId, selected.sha256)} download className="text-brand-700 hover:underline">Download</a>
              )}
            </div>
            {error && <p className="px-3 py-2 text-ui text-err">{error}</p>}
            {text !== null && <pre className="max-h-96 overflow-auto px-3 py-2 text-note font-mono text-ink whitespace-pre">{text}</pre>}
            {selected.kind !== 'text' && !selected.host && <p className="px-3 py-2 text-ui text-ink-mute">Binary file. Use Download.</p>}
          </>
        )}
      </div>
    </div>
  );
}
```
(The test for the "on host" label expects the external-with-host artifact's selection to show `stored on host …`; make the assertion match the rendered text.)

`CrossReferences.tsx`:
```tsx
import { Link } from 'react-router-dom';
import Markdown from '../../transcript/Markdown';
import { isSentinel, type CrossRef, type OrSentinel } from '../../../lib/findingReport';

export default function CrossReferences({ refs, references }: { refs: OrSentinel<CrossRef[]>; references: string }) {
  return (
    <div className="space-y-2">
      <div className="text-ink-dim"><Markdown text={references} /></div>
      <div className="text-ui text-ink-dim">
        <span className="font-semibold text-ink">Related findings: </span>
        {isSentinel(refs) ? refs : (
          <ul className="mt-1 space-y-0.5">
            {refs.map((r) => (
              <li key={r.finding_id}>
                <span className="text-ink-mute">{r.relation}</span>{' '}
                <Link to={`/findings/${encodeURIComponent(r.finding_id)}`} className="font-mono text-brand-700 hover:underline">{r.finding_id}</Link>
                {r.note && <span className="text-ink-mute"> — {r.note}</span>}
              </li>
            ))}
          </ul>
        )}
      </div>
    </div>
  );
}
```

`ReportHeader.tsx`:
```tsx
import SeverityChip from '../../coverage/SeverityChip';
import { normFindingSeverity, type FindingOut } from '../../../lib/api';
import { isGapSentinel, isSentinel, type FindingReport } from '../../../lib/findingReport';

function Cell({ label, value }: { label: string; value: string }) {
  const gap = isGapSentinel(value);
  return (
    <>
      <dt className="text-ui text-ink-mute">{label}</dt>
      <dd data-gap={gap ? 'true' : 'false'} className={`min-w-0 break-words text-ui ${gap ? 'text-warn' : 'text-ink'}`}>{value}</dd>
    </>
  );
}

export default function ReportHeader({ finding, report }: { finding: FindingOut; report: FindingReport }) {
  const r = report.rating;
  const tickets = isSentinel(report.tickets) ? report.tickets : report.tickets.map((t) => `${t.type} ${t.identifier}`).join(', ');
  return (
    <header className="space-y-3">
      <div className="font-mono text-note text-ink-mute">{finding.project || finding.ws_id} · {finding.workflow_name ?? 'finding'} · {finding.id}</div>
      <h1 className="text-2xl font-semibold leading-tight text-ink">{report.title}</h1>
      <div className="flex flex-wrap items-center gap-1.5">
        <SeverityChip severity={normFindingSeverity(r.risk_rating)} />
        {(report.cwe ?? []).map((c) => {
          const n = c.replace(/^CWE-/, '');
          return (
            <a key={c} href={`https://cwe.mitre.org/data/definitions/${n}.html`} target="_blank" rel="noreferrer"
               className="rounded bg-surface px-1.5 py-0.5 text-note font-medium text-ink ring-1 ring-border hover:bg-surface-hover">{c}</a>
          );
        })}
        {report.verification && <span className="rounded bg-surface px-1.5 py-0.5 text-note text-ink-dim ring-1 ring-border">verification: {report.verification.status}</span>}
        {(report.artifacts?.length ?? 0) > 0 && <span className="rounded bg-ok-bg px-1.5 py-0.5 text-note text-ok">PoC artifacts: {report.artifacts!.length}</span>}
      </div>
      <div className="grid gap-0 overflow-hidden rounded-md border border-border bg-panel md:grid-cols-2">
        <dl className="grid grid-cols-[8rem_minmax(0,1fr)] gap-x-3 gap-y-1 px-4 py-3 md:border-r md:border-border">
          <Cell label="Owner" value={report.ownership.owner} />
          <Cell label="Product" value={report.ownership.product} />
          <Cell label="Component" value={report.ownership.affected_component} />
          <Cell label="Source repo" value={report.ownership.source_repository} />
          <Cell label="Tickets" value={tickets} />
        </dl>
        <dl className="grid grid-cols-[8rem_minmax(0,1fr)] gap-x-3 gap-y-1 px-4 py-3">
          <Cell label="Category" value={report.category} />
          <Cell label="Attack vector" value={report.attack_vector} />
          <Cell label="Impact" value={r.impact} />
          <Cell label="Likelihood" value={r.likelihood} />
          <Cell label="Risk rating" value={r.risk_rating} />
          <Cell label="Risk factor" value={r.risk_factor} />
          <Cell label="CVSS v3" value={r.cvss_v3} />
        </dl>
      </div>
    </header>
  );
}
```
(`normFindingSeverity` takes a raw string. Check that it lowercases "Critical". If it doesn't, pass `r.risk_rating.toLowerCase()`.)

- [ ] **Step 4: Run and check it passes**

Run: `cd crates/rupu-cp/web && npx vitest run src/components/findings/report && npx tsc --noEmit -p .`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-cp/web/src/components/findings/report
git commit -m "feat(cp-web): finding report section components

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 6: `/findings/:id` page

**Files:**
- Create: `crates/rupu-cp/web/src/pages/FindingDetail.tsx`, `crates/rupu-cp/web/src/pages/FindingDetail.test.tsx`
- Modify: `crates/rupu-cp/web/src/App.tsx` (lazy import + route next to `/findings`), `crates/rupu-cp/web/src/lib/paletteSources.ts` (`findingItems` → `/findings/<id>`; drop the "no detail route" comment), `crates/rupu-cp/web/src/components/CommandPalette.tsx` (only if its `V2_LIST_REWRITE` would rewrite `/findings/<id>` back to the list; exempt detail paths)

**Interfaces:**
- Consumes: `api.getFinding`, Task 5 components, `FindingEvidence` (for summary findings), `Markdown`, `ErrorBanner`, `Spinner`, `EmptyState`.
- Produces: the route `/findings/:id`, served by the `FindingDetail` page.

- [ ] **Step 1: Write the failing test** — `pages/FindingDetail.test.tsx`. Render inside `<MemoryRouter initialEntries={['/findings/fnd_1']}><Routes><Route path="/findings/:id" element={<FindingDetail />} /></Routes></MemoryRouter>` and mock with `vi.spyOn(api, 'getFinding')`. Cases:
  - **Full profile** (report = fixture, `evidence_status: ['current']`, `ws_id: 'ws1'`). It shows:
    - the title as the `h1`;
    - the root cause text;
    - section headings "Call chain", "Evidence", "Recommended patch", "Regression test" and "Replication steps";
    - a rail link `href="#s-root"`;
    - the completeness text `9/11` with gaps "owner" and "cvss_v3" listed. Completeness is computed client-side by the ported function below.
  - **Summary profile** (`report: null`, `profile: 'summary'`). It shows the summary text, the rationale via `FindingEvidence`, and the note "This finding was recorded as a summary".
  - **API 404.** It shows an `ErrorBanner` (`role="alert"`) containing the error text.

- [ ] **Step 2: Run and check it fails**

Run: `cd crates/rupu-cp/web && npx vitest run src/pages/FindingDetail.test.tsx`
Expected: FAIL.

- [ ] **Step 3: Implement.** Add `completeness(report)` to `lib/findingReport.ts`, ported exactly from Task 1 (same 11 fields, same rules), with a unit test in `findingReport.test.ts` asserting the fixture gives `{filled: 9, total: 11, gaps: ['owner','cvss_v3']}`:

```ts
export function completeness(r: FindingReport): Completeness {
  const unknown = (s: string) => s.trim() === 'Unknown';
  const notProvided = (v: OrSentinel<unknown>) => typeof v === 'string' && v.startsWith(NOT_PROVIDED);
  const checks: [string, boolean][] = [
    ['owner', unknown(r.ownership.owner)],
    ['product', unknown(r.ownership.product)],
    ['affected_component', unknown(r.ownership.affected_component)],
    ['source_repository', unknown(r.ownership.source_repository)],
    ['tickets', typeof r.tickets === 'string' && unknown(r.tickets)],
    ['cvss_v3', unknown(r.rating.cvss_v3)],
    ['attack_vector', unknown(r.attack_vector)],
    ['call_chain', notProvided(r.call_chain)],
    ['recommended_patch', notProvided(r.recommended_patch)],
    ['ci_cd_detection', notProvided(r.ci_cd_detection)],
    ['regression_test', notProvided(r.regression_test)],
  ];
  const gaps = checks.filter(([, g]) => g).map(([n]) => n);
  return { filled: checks.length - gaps.length, total: checks.length, gaps };
}
```

`pages/FindingDetail.tsx`:
```tsx
import { useEffect, useState } from 'react';
import { Link, useParams } from 'react-router-dom';
import { api, type FindingDetail as Detail } from '../lib/api';
import { completeness } from '../lib/findingReport';
import Markdown from '../components/transcript/Markdown';
import { FindingEvidence } from '../components/findings/FindingEvidence';
import Section from '../components/findings/report/Section';
import ReportHeader from '../components/findings/report/ReportHeader';
import CallChain from '../components/findings/report/CallChain';
import EvidenceClaims from '../components/findings/report/EvidenceClaims';
import FixSections from '../components/findings/report/FixSections';
import ReplicationSteps from '../components/findings/report/ReplicationSteps';
import ArtifactBrowser from '../components/findings/report/ArtifactBrowser';
import CrossReferences from '../components/findings/report/CrossReferences';
import { ErrorBanner } from '../components/ui/ErrorBanner';
import { Spinner } from '../components/ui/Spinner';

const RAIL: [string, string][] = [
  ['s-desc', 'Description'], ['s-impact', 'Impact'], ['s-loc', 'Location'], ['s-root', 'Root cause'],
  ['s-chain', 'Call chain'], ['s-evidence', 'Evidence'], ['s-artifacts', 'PoC artifacts'], ['s-repro', 'Replication'],
  ['s-remediation', 'Remediation'], ['s-patch', 'Patch'], ['s-ci', 'CI/CD detection'], ['s-reg', 'Regression test'],
  ['s-refs', 'References'], ['s-prov', 'Provenance'],
];

export default function FindingDetail() {
  const { id = '' } = useParams();
  const [detail, setDetail] = useState<Detail | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let live = true;
    setDetail(null);
    setError(null);
    api.getFinding(id).then((d) => live && setDetail(d), (e: unknown) => live && setError(e instanceof Error ? e.message : String(e)));
    return () => { live = false; };
  }, [id]);

  if (error) return <div className="p-8"><ErrorBanner>{error}</ErrorBanner></div>;
  if (!detail) return <div className="p-8"><Spinner label="Loading finding" /></div>;

  const report = detail.report;
  const prov = (
    <Section id="s-prov" title="Provenance">
      <dl className="grid gap-x-6 gap-y-1 text-ui text-ink-dim sm:grid-cols-2">
        <div><dt className="text-note text-ink-mute">Finding</dt><dd className="font-mono">{detail.id}</dd></div>
        <div><dt className="text-note text-ink-mute">Run</dt><dd><Link className="font-mono text-brand-700 hover:underline" to={`/runs/${encodeURIComponent((detail.declared_by as { run_id: string }).run_id)}`}>{(detail.declared_by as { run_id: string }).run_id}</Link></dd></div>
        <div><dt className="text-note text-ink-mute">Model</dt><dd>{(detail.declared_by as { model: string }).model}</dd></div>
        <div><dt className="text-note text-ink-mute">Declared</dt><dd>{new Date(detail.declared_at).toLocaleString()}</dd></div>
      </dl>
    </Section>
  );

  if (!report) {
    return (
      <div className="mx-auto max-w-4xl space-y-6 p-8">
        <h1 className="text-2xl font-semibold text-ink">{detail.summary}</h1>
        <p className="text-ui text-ink-mute">This finding was recorded as a summary, without a full report.</p>
        <FindingEvidence finding={detail} />
        {prov}
      </div>
    );
  }

  const c = completeness(report);
  return (
    <div className="grid gap-8 p-8 lg:grid-cols-[13rem_minmax(0,1fr)]">
      <nav aria-label="Report sections" className="hidden self-start lg:sticky lg:top-4 lg:block">
        <div className="mb-3 rounded-md border border-border bg-panel p-3">
          <div className="flex justify-between text-note"><span className="font-semibold text-ink">Report</span><span className="font-mono text-ink-dim">{c.filled}/{c.total}</span></div>
          <div className="mt-1.5 h-1.5 overflow-hidden rounded bg-surface"><div className="h-full bg-ok" style={{ width: `${(c.filled / c.total) * 100}%` }} /></div>
          {c.gaps.length > 0 && <p className="mt-1.5 text-note text-ink-mute">Unknown: {c.gaps.join(', ')}</p>}
        </div>
        <ul className="space-y-px">
          {RAIL.map(([anchor, label]) => (
            <li key={anchor}><a href={`#${anchor}`} className="block border-l-2 border-border px-2 py-0.5 text-note text-ink-mute hover:border-brand-500 hover:text-ink">{label}</a></li>
          ))}
        </ul>
      </nav>
      <article className="min-w-0 max-w-4xl space-y-7">
        <ReportHeader finding={detail} report={report} />
        <Section id="s-desc" title="Description"><div className="text-ink-dim"><Markdown text={report.description} /></div></Section>
        <Section id="s-impact" title="Impact"><div className="text-ink-dim"><Markdown text={report.impact} /></div></Section>
        <Section id="s-loc" title="Location">
          <div className="grid gap-2 sm:grid-cols-2">
            <div className="rounded-md border border-border bg-panel px-3 py-2"><div className="text-meta uppercase tracking-wide text-ink-mute">Input</div><Markdown text={report.location.input} /></div>
            <div className="rounded-md border border-border bg-panel px-3 py-2"><div className="text-meta uppercase tracking-wide text-ink-mute">Output</div><Markdown text={report.location.output} /></div>
          </div>
        </Section>
        <Section id="s-root" title="Root cause"><div className="rounded-md border border-brand-500 bg-brand-50 px-4 py-3 text-ink"><Markdown text={report.root_cause} /></div></Section>
        <Section id="s-chain" title="Call chain"><CallChain chain={report.call_chain} wsId={detail.ws_id} /></Section>
        <Section id="s-evidence" title="Evidence"><EvidenceClaims claims={report.evidence} states={detail.evidence_status} wsId={detail.ws_id} /></Section>
        {(report.artifacts?.length ?? 0) > 0 && <Section id="s-artifacts" title="PoC artifacts"><ArtifactBrowser findingId={detail.id} artifacts={report.artifacts!} /></Section>}
        <Section id="s-repro" title="Replication steps"><ReplicationSteps steps={report.replication_steps} /></Section>
        <Section id="s-remediation" title="Remediation"><div className="text-ink-dim"><Markdown text={report.remediation} /></div></Section>
        <FixSections report={report} />
        <Section id="s-refs" title="References"><CrossReferences refs={report.cross_references} references={report.references} /></Section>
        {prov}
      </article>
    </div>
  );
}
```
(Check the real `ErrorBanner` and `Spinner` exports and props, which may be named/default or take `message`/`label`, and match them. `declared_by` is typed `unknown` in `FindingRecord`. Tighten it to `{ run_id: string; model: string; surface: string }` in `api.ts` if nothing breaks, and drop the casts.)

In `App.tsx`: add `const FindingDetail = React.lazy(() => import('./pages/FindingDetail'));` and `<Route path="/findings/:id" element={page(<FindingDetail />)} />`, placed next to the `/findings` route. It must not be redirected under the v2 shell.

In `paletteSources.ts` `findingItems`: set `to: `/findings/${encodeURIComponent(f.id)}``. Update its test if one asserts the old `/findings` target.

- [ ] **Step 4: Run and check it passes**

Run: `cd crates/rupu-cp/web && npx vitest run src/pages/FindingDetail.test.tsx src/lib src/components/CommandPalette* && npx tsc --noEmit -p .`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-cp/web/src
git commit -m "feat(cp-web): /findings/:id report page with section rail and completeness meter

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 7: Findings tables — triage card, Report column, filters

**Files:**
- Create: `crates/rupu-cp/web/src/components/findings/TriageCard.tsx`, `crates/rupu-cp/web/src/components/findings/TriageCard.test.tsx`
- Modify: `crates/rupu-cp/web/src/components/findings/FindingsTable.tsx`, `crates/rupu-cp/web/src/pages/Findings.tsx` (+ its test), `crates/rupu-cp/web/src/components/findings/FindingRow.tsx` (add an "Open report" link when `finding.profile === 'full'`)

**Interfaces:**
- Consumes: `report_summary` on list rows (Task 2/4), `FindingEvidence`, `codeHref`.
- Produces:
  - `TriageCard({ finding })`: the expanded-row body for full-profile rows.
  - A `Report` column in `FindingsTable`.
  - Profile / Owner / CWE filters on the global Findings page.

- [ ] **Step 1: Write the failing tests**
  - `TriageCard.test.tsx` with a `FindingRecord` carrying `profile: 'full'` and `report_summary` (root_cause, chain `['a','b','sink']`, owner 'Unknown', product 'Notebin', completeness `{filled:9,total:11,gaps:['owner','cvss_v3']}`, has_poc true). It renders:
    - the root cause (markdown);
    - the chain joined with `→`;
    - "Owner: Unknown" with a gap style;
    - the gap names;
    - a link "Open full report" to `/findings/<id>`.
  - Extend `FindingsTable.deeplink.test.tsx` (or add `FindingsTable.report.test.tsx`):
    - A full-profile row's expanded detail renders `TriageCard`; a summary row still renders `FindingEvidence`.
    - The `Report` column shows `9/11` for the full row, plus "PoC" when `has_poc`, and `summary` for the summary row.
  - In `pages/Findings.test.tsx`:
    - Selecting the "Full reports" profile pill hides summary rows.
    - Selecting an owner from the Owner select keeps only matching rows.
    - Selecting CWE-639 keeps only rows whose `report_summary.cwe` or `cweFromFinding` matches.

- [ ] **Step 2: Run and check they fail**

Run: `cd crates/rupu-cp/web && npx vitest run src/components/findings src/pages/Findings.test.tsx`
Expected: FAIL.

- [ ] **Step 3: Implement**

`TriageCard.tsx`:
```tsx
import { Link } from 'react-router-dom';
import Markdown from '../transcript/Markdown';
import type { FindingRecord } from '../../lib/api';

export default function TriageCard({ finding }: { finding: FindingRecord }) {
  const s = finding.report_summary;
  if (!s) return null;
  const gap = (v: string) => v.trim() === 'Unknown';
  return (
    <div className="grid gap-4 md:grid-cols-[minmax(0,1.4fr)_minmax(0,1fr)]">
      <div className="min-w-0 space-y-2">
        <h4 className="text-meta font-semibold uppercase tracking-wide text-ink-mute">Root cause</h4>
        <div className="text-ink-dim [&_p]:text-ui"><Markdown text={s.root_cause} /></div>
        {s.chain.length > 0 && (
          <>
            <h4 className="text-meta font-semibold uppercase tracking-wide text-ink-mute">Attack path</h4>
            <p className="font-mono text-note text-ink-dim">{s.chain.join(' → ')}</p>
          </>
        )}
      </div>
      <div className="min-w-0 space-y-2">
        <p className="text-ui">
          <span className={gap(s.owner) ? 'text-warn' : 'text-ink'} data-gap={gap(s.owner) ? 'true' : 'false'}>Owner: {s.owner}</span>
          <span className="text-ink-mute"> · </span>
          <span className={gap(s.product) ? 'text-warn' : 'text-ink'}>{s.product}</span>
        </p>
        <p className="text-note text-ink-mute">
          Report {s.completeness.filled}/{s.completeness.total}
          {s.completeness.gaps.length > 0 && <> · unknown: {s.completeness.gaps.join(', ')}</>}
          {s.has_poc && <> · PoC attached</>}
          {s.verification_status && <> · verification: {s.verification_status}</>}
        </p>
        <Link to={`/findings/${encodeURIComponent(finding.id)}`} className="inline-block rounded-md bg-brand-50 px-2.5 py-1 text-ui font-medium text-brand-700 hover:bg-brand-100">
          Open full report →
        </Link>
      </div>
    </div>
  );
}
```

In `FindingsTable.tsx`:
- change `renderDetail` to `(f) => (f.profile === 'full' && f.report_summary ? <TriageCard finding={f} /> : <FindingEvidence finding={f} />)`;
- add, after the `summary` column:
```tsx
    {
      key: 'report',
      header: 'Report',
      fit: true,
      sortable: true,
      sortValue: (f) => (f.report_summary ? f.report_summary.completeness.filled / f.report_summary.completeness.total : -1),
      render: (f) =>
        f.report_summary ? (
          <span className="font-mono text-note text-ink-dim">
            {f.report_summary.completeness.filled}/{f.report_summary.completeness.total}
            {f.report_summary.has_poc && <span className="ml-1 text-ok">PoC</span>}
          </span>
        ) : (
          <span className="text-note text-ink-mute">summary</span>
        ),
    },
```
- in the CWE column, prefer `f.report_summary?.cwe[0]` when present: build `{id, url}` from it the same way `cweFromFinding` does. Otherwise keep `cweFromFinding(f)`.
- replace the hand-built code URL with `codeHref(rowWsId, f.file_path!, f.line_range![0])`.

In `pages/Findings.tsx`, add client-side filters next to the severity metrics, using the existing `FilterPills` and `Select` primitives (`components/ui/`, see `pages/Sessions.tsx:151` for usage):
- Profile: All / Full reports / Summaries (filters on `f.profile ?? 'summary'`).
- Owner: a select with "All owners" plus the distinct `report_summary.owner` values.
- CWE: a select with "All CWEs" plus the distinct CWE ids from `report_summary.cwe` and `cweFromFinding`.

All filters combine with the existing severity filter. The metrics tiles keep showing totals for the unfiltered set.

In `FindingRow.tsx` (the run detail list): when `finding.profile === 'full'`, render a small `Link` "Open report" to `/findings/<id>` next to the evidence toggle.

- [ ] **Step 4: Run and check they pass**

Run: `cd crates/rupu-cp/web && npx vitest run src/components/findings src/pages && npx tsc --noEmit -p .`
Expected: PASS, except the known `AutoflowRuns.columnOrder` failure.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-cp/web/src
git commit -m "feat(cp-web): findings triage card, report column, profile/owner/CWE filters

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 8: Code tab inline card — tabbed report view

**Files:**
- Modify: `crates/rupu-cp/web/src/components/code/InlineFindingCard.tsx` (+ create `InlineFindingCard.report.test.tsx`)

**Interfaces:**
- Consumes: `api.getFinding`, `CallChain`, `EvidenceClaims`, `FixSections`'s patch rendering (reuse `DiffView` directly), `ReplicationSteps`, `codeHref`.
- Produces: when `finding.profile === 'full'` and the card is expanded, it lazily fetches `api.getFinding(finding.id)` and shows these tabs:
  - **Root cause** (default)
  - **Call chain**
  - **Evidence** (with jump links)
  - **Patch**
  - **Repro**

  It also shows an "Open full report →" link. Summary-profile findings render exactly as today.

- [ ] **Step 1: Write the failing test.** Render `InlineFindingCard` with a full-profile finding inside `MemoryRouter`, and mock `vi.spyOn(api,'getFinding')` to resolve a detail with the fixture report.
  - After clicking the collapsed header, the "Root cause" tab content shows the root-cause text.
  - Clicking the "Call chain" tab shows the hop labels.
  - The "Open full report" link points at `/findings/<id>`.
  - `getFinding` is not called before expanding.
  - A summary-profile finding never calls `getFinding`.

- [ ] **Step 2: Run and check it fails**

Run: `cd crates/rupu-cp/web && npx vitest run src/components/code/InlineFindingCard.report.test.tsx`
Expected: FAIL.

- [ ] **Step 3: Implement.** Inside `InlineFindingCard`, keep the current markup for summary findings. For `finding.profile === 'full'`, add these to the expanded body:

```tsx
const TABS = ['Root cause', 'Call chain', 'Evidence', 'Patch', 'Repro'] as const;
type Tab = (typeof TABS)[number];
// state
const [detail, setDetail] = useState<FindingDetail | null>(null);
const [tab, setTab] = useState<Tab>('Root cause');
const [loadError, setLoadError] = useState<string | null>(null);
useEffect(() => {
  if (!open || finding.profile !== 'full' || detail) return;
  let live = true;
  api.getFinding(finding.id).then((d) => live && setDetail(d), (e: unknown) => live && setLoadError(e instanceof Error ? e.message : String(e)));
  return () => { live = false; };
}, [open, finding.profile, finding.id, detail]);
```

Render:
- a tab row (`role="tablist"`, buttons `role="tab"` with `aria-selected`);
- the active panel:
  - Root cause: `Markdown` of `report.root_cause`;
  - Call chain: `<CallChain chain=… wsId={(finding as FindingOut).ws_id} />`;
  - Evidence: `<EvidenceClaims claims=… states={detail.evidence_status} wsId=… />`;
  - Patch: `DiffView` when `recommended_patch` isn't a sentinel, else `sentinelLabel`;
  - Repro: `<ReplicationSteps steps=… />`;
- a footer `Link` "Open full report →" to `/findings/<id>`;
- a `Spinner` while `detail` is null, and the error text if loading failed.

Keep the existing stale banner and permalink.

- [ ] **Step 4: Run and check it passes**

Run: `cd crates/rupu-cp/web && npx vitest run src/components/code && npx tsc --noEmit -p .`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-cp/web/src/components/code
git commit -m "feat(cp-web): tabbed report view in the Code tab's inline finding card

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 9: Workflow editor — `findings_profile` on steps and in defaults

**Files:**
- Modify: `crates/rupu-cp/web/src/lib/workflowGraph.ts` (model/parse/emit/validate), `crates/rupu-cp/web/src/components/workflow-editor/StepForm.tsx`, `crates/rupu-cp/web/src/lib/workflowMeta.ts` (`readDefaults`/`writeDefaults`), `crates/rupu-cp/web/src/components/workflow-editor/WorkflowSettingsForm.tsx`
- Create: `crates/rupu-cp/web/src/components/workflow-editor/settings/DefaultsCard.tsx` (+ test), plus tests in the existing `workflowGraph`/`StepForm` test files

**Interfaces:**
- Produces:
  - `StepNodeData.findings_profile?: 'full' | 'summary'`
  - `readDefaults(rest): { findings_profile?: 'full' | 'summary' }`
  - `writeDefaults(rest, model): Record<string, unknown>`. It preserves other `defaults` keys (`continue_on_error`, `workspace`) and deletes `defaults` when it ends up empty.
  - `DefaultsCard({ rest, onRest })`
  - new `validateGraph` problems mirroring the server's parse rules (text must match the server messages' meaning):
    - a remote step (`host` or `distribute` set) with `findings_profile` → "findings_profile is not supported on a remote step; set findingsProfile in the agent's frontmatter";
    - a step kind that runs no agent (branch, gate/approval-only, `run:`, bare split/join) with `findings_profile` → "findings_profile has no effect on a step that runs no agent";
    - `defaults.findings_profile` set while any step is remote → "defaults.findings_profile does not reach remote step <id>".

  `action:` steps may carry `findings_profile`: the Plan 1 final-review fix allows it and plumbs it per call. Verify this against `crates/rupu-orchestrator/src/workflow.rs` on the branch, and mirror whatever the server does.

- [ ] **Step 1: Write the failing tests**
  - **workflowGraph round-trip.** YAML with `findings_profile: summary` on a linear agent step parses into `data.findings_profile === 'summary'`, is no longer in `raw_passthrough`, and re-emits identically. A step without it emits no key.
  - **validateGraph.** It returns the new problems for a `host:` step with a profile, and for a branch step with a profile. It returns none for a local agent step with a profile.
  - **workflowMeta.** `writeDefaults({defaults:{continue_on_error:true}}, {findings_profile:'summary'})` gives `defaults: {continue_on_error: true, findings_profile: 'summary'}`. `writeDefaults({defaults:{findings_profile:'full'}}, {})` deletes `defaults`. `readDefaults` ignores unknown values.
  - **StepForm.** For an agent step, the "Findings" select offers "Inherit", "Full report" and "Summary". Choosing "Summary" calls `onChange` with `findings_profile: 'summary'`, and choosing "Inherit" clears it. The select is not rendered for a branch step.
  - **DefaultsCard.** It renders a "Default findings profile" select that writes through `onRest`, and it lists any other `defaults` keys read-only.

- [ ] **Step 2: Run and check they fail**

Run: `cd crates/rupu-cp/web && npx vitest run src/lib/workflowGraph src/lib/workflowMeta src/components/workflow-editor`
Expected: FAIL.

- [ ] **Step 3: Implement**, following the survey of this code:
  - **`workflowGraph.ts`**
    - Add `findings_profile?: 'full' | 'summary'` to `StepNodeData`.
    - In `parseStepData`: `const fp = asString(o.findings_profile); if (fp === 'full' || fp === 'summary') data.findings_profile = fp;`
    - Add `'findings_profile'` to `MODELLED_STEP_KEYS`.
    - In `nodeToStepObject`, emit it kind-independently next to the `next` handling: `if (d.findings_profile) o.findings_profile = d.findings_profile;`
    - Add the three checks to `validateGraph`, reusing its existing problem shape. The defaults check needs the workflow meta; if `validateGraph` has no access to `meta.rest`, add an optional parameter and pass it from `WorkflowEditor.tsx`.
  - **`StepForm.tsx`**
    - In the common block (the one with "When" and "Continue on error", ~lines 240-283), render this only for node kinds that run an agent (agent / for_each / parallel / panel) and for action steps:
    ```tsx
    <label className="block">
      <span className={labelCls}>Findings</span>
      <select
        aria-label="Findings profile"
        value={data.findings_profile ?? ''}
        onChange={(e) => patch({ findings_profile: (e.target.value || undefined) as 'full' | 'summary' | undefined })}
        className={fieldCls}
      >
        <option value="">Inherit (workflow default → agent → full)</option>
        <option value="full">Full report</option>
        <option value="summary">Summary</option>
      </select>
    </label>
    ```
    - Use the real kind discriminant names from `GraphNode`/`StepNodeData`.
    - In `switchKind`, carry `findings_profile` only into kinds that accept it.
  - **`workflowMeta.ts`**: add `readDefaults`/`writeDefaults` in the style of `readTrigger`/`writeTrigger`, using the existing `asRecord`, `asString` and `setOrDeleteKey` helpers. Preserve key order and every other key under `defaults`.
  - **`DefaultsCard.tsx`**: model it on `settings/TriggerCard.tsx`, the smallest card. Render one select, "Default findings profile" (Agent decides / Full report / Summary), plus a read-only line listing other `defaults` keys ("Other defaults (edit in YAML): …").
  - **`WorkflowSettingsForm.tsx`**: render `<DefaultsCard rest={meta.rest} onRest={onRest} />` after `InputsCard`, and add `'defaults'` to the keys excluded from the "Preserved advanced keys" chips.

- [ ] **Step 4: Run and check they pass**

Run: `cd crates/rupu-cp/web && npx vitest run src/lib src/components/workflow-editor && npx tsc --noEmit -p .`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-cp/web/src
git commit -m "feat(cp-web): findings_profile in the workflow editor (step field, defaults card, validation)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 10: Docs + gate

**Files:**
- Modify: `docs/coverage.md` (Finding reports section: the CP report page, list triage card, Code-tab card, artifact endpoint rules; keep the note that Markdown/HTML/PDF exports come in a later release), `docs/workflow-format.md` (the editor supports `findings_profile`), `CLAUDE.md` (add Plan 2 to "Read first"; one sentence in the `rupu-cp` crate entry about `GET /api/findings/:id` and `/api/findings/:id/artifacts/:sha256`)

- [ ] **Step 1: Update the docs** as listed. Keep them factual and don't promise exports.

- [ ] **Step 2: Run the gate** (each separately; record the results):

```bash
cargo test -p rupu-coverage -p rupu-cp
```
```bash
cargo clippy --workspace --all-targets -- -D warnings -A clippy::question_mark
```
```bash
cd crates/rupu-cp/web && npx vitest run && npx tsc --noEmit -p .
```
```bash
make cp-web
```
```bash
make macos-fixtures && git status --short apps/rupu-macos/Fixtures
```
Expected: green, apart from the known `AutoflowRuns.columnOrder` web failure. No fixture drift, or drift that is committed.

- [ ] **Step 3: Commit**

```bash
git add docs CLAUDE.md
git commit -m "docs: finding report page, artifact endpoint, editor support

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

- [ ] **Step 4: GUI check handoff.** matt opens the Findings page, a full-profile finding's report page, and the Code tab for a file with a full-profile finding, and edits a workflow's findings profile. Nothing merges before that.
