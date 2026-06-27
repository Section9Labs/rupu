# CP Web Run — Sub-project A Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Let a user start a workflow run from the CP web UI (a Run button + modal on WorkflowDetail), launched via the existing `RunLauncher` port, with the spawned run fully detached so it survives the CP closing.

**Architecture:** The backend launch + cancel machinery already exists (`rupu-cp` `RunLauncher` port; `rupu-cli` `SubprocessLauncher` adapter installed by `cp serve`; `RunStore::cancel` + cancel endpoint + RunDetail Cancel UI all done). This sub-project closes the remaining gaps: detach the spawned child, add the dispatch HTTP endpoint, and build the Run form.

**Tech Stack:** Rust + axum (rupu-cp), Rust (rupu-cli), React 18 + TS + Vite + Vitest + Tailwind (web).

Spec: `docs/superpowers/specs/2026-06-26-rupu-cp-web-run-design.md` (see "Already built" + "Sub-project A — gaps to close").

## Global Constraints
- `rupu-cp` is thin: the dispatch handler only validates + delegates to the `RunLauncher` port; no run logic in-process.
- `#![deny(clippy::all)]`; `unsafe_code` forbidden; per-file `rustfmt` only (never package-wide).
- Web: pure-logic tests node env; component tests `// @vitest-environment jsdom` with `afterEach(cleanup)` (this repo runs vitest `globals: false`).
- Existing types to reuse (do NOT redefine): `rupu_cp::launcher::{LaunchRequest, LaunchError, RunLauncher}`; `LaunchRequest { workflow: String, inputs: BTreeMap<String,String>, mode: Option<String>, target: Option<String> }`; `ApiError::{not_found, bad_request, internal, not_available}`.

---

## Task A1: Detach the spawned run (survives CP close)

**Files:**
- Modify: `crates/rupu-cli/src/cp_launcher.rs`

**Interfaces:**
- No signature change: `SubprocessLauncher::launch` still returns `run_<ULID>`.
  `build_run_argv` is unchanged (its tests stay green).

- [ ] **Step 1: Update the spawn to fully detach**

Replace the `launch` impl body's spawn block. Add imports at the top of the file:

```rust
use std::process::Stdio;
#[cfg(unix)]
use std::os::unix::process::CommandExt;
```

Change the spawn in `launch`:

```rust
    async fn launch(&self, req: LaunchRequest) -> Result<String, LaunchError> {
        let run_id = format!("run_{}", ulid::Ulid::new());
        let argv = build_run_argv(&req, &run_id);
        // Detached: its own process group + null stdio, so a Ctrl-C / SIGINT to
        // `cp serve` (or the CP exiting) does not take the run down. The child
        // writes its own run.json / events.jsonl / transcripts.
        let mut cmd = std::process::Command::new(&self.exe);
        cmd.args(&argv)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(unix)]
        cmd.process_group(0); // new session/group; detaches from cp-serve's
        cmd.spawn().map_err(|e| LaunchError::Spawn(e.to_string()))?;
        Ok(run_id)
    }
```

- [ ] **Step 2: Verify build + existing argv tests still pass**

Run: `cargo test -p rupu-cli --lib cp_launcher`
Expected: PASS (the two `build_run_argv` tests unchanged).
Run: `cargo clippy -p rupu-cli --lib` → clean (note: per project memory rupu-cli may show pre-existing toolchain noise under Homebrew rustc; only this file's warnings matter).

- [ ] **Step 3: Commit**

```bash
git add crates/rupu-cli/src/cp_launcher.rs
git commit -m "feat(cp): detach spawned runs (own process group + null stdio)"
```

---

## Task A2: Dispatch endpoint

**Files:**
- Modify: `crates/rupu-cp/src/api/workflows.rs`

**Interfaces:**
- Produces: `POST /api/workflows/:name/dispatch`, body
  `{ inputs?: map<string,string>, mode?: string, target?: string }` →
  `{ "run_id": string }`. 501 when no launcher; 404 unknown workflow; 400 bad
  mode / `LaunchError::Invalid`; 500 `LaunchError::Spawn`.

- [ ] **Step 1: Write the failing test**

At the bottom of `crates/rupu-cp/src/api/workflows.rs` add a test module. It
exercises the pure validation+mapping via a small mock launcher. (Construct the
handler inputs directly; do not boot axum.)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use rupu_cp_test_support as _; // no-op if absent; remove if unused
    use std::sync::Arc;

    struct MockLauncher {
        last: std::sync::Mutex<Option<crate::launcher::LaunchRequest>>,
    }
    #[async_trait::async_trait]
    impl crate::launcher::RunLauncher for MockLauncher {
        async fn launch(
            &self,
            req: crate::launcher::LaunchRequest,
        ) -> Result<String, crate::launcher::LaunchError> {
            *self.last.lock().unwrap() = Some(req);
            Ok("run_TEST".to_string())
        }
    }

    #[test]
    fn valid_mode_accepted_invalid_rejected() {
        assert!(validate_mode(None).is_ok());
        assert!(validate_mode(Some("ask")).is_ok());
        assert!(validate_mode(Some("bypass")).is_ok());
        assert!(validate_mode(Some("readonly")).is_ok());
        assert!(validate_mode(Some("nope")).is_err());
    }

    #[tokio::test]
    async fn dispatch_forwards_request_to_launcher() {
        let mock = Arc::new(MockLauncher { last: Default::default() });
        let body = DispatchBody {
            inputs: std::collections::BTreeMap::from([("k".into(), "v".into())]),
            mode: Some("bypass".into()),
            target: Some("github:o/r".into()),
        };
        let run_id = dispatch_with_launcher("audit", body, mock.clone())
            .await
            .expect("ok");
        assert_eq!(run_id, "run_TEST");
        let req = mock.last.lock().unwrap().clone().unwrap();
        assert_eq!(req.workflow, "audit");
        assert_eq!(req.mode.as_deref(), Some("bypass"));
        assert_eq!(req.target.as_deref(), Some("github:o/r"));
        assert_eq!(req.inputs.get("k").map(String::as_str), Some("v"));
    }
}
```

(Delete the `rupu_cp_test_support` line — it's a placeholder reminder; no such
crate. Keep the test self-contained.)

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rupu-cp --lib api::workflows`
Expected: FAIL — `validate_mode` / `dispatch_with_launcher` / `DispatchBody` not found.

- [ ] **Step 3: Implement**

Update the imports + routes at the top of `workflows.rs`:

```rust
use crate::launcher::{LaunchError, LaunchRequest, RunLauncher};
use std::collections::BTreeMap;
use std::sync::Arc;
```

```rust
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/workflows", get(list_workflows))
        .route("/api/workflows/:name", get(get_workflow))
        .route("/api/workflows/:name/dispatch", axum::routing::post(dispatch_workflow))
}
```

Add the body type, validation, the launcher-agnostic core, and the handler:

```rust
#[derive(serde::Deserialize)]
struct DispatchBody {
    #[serde(default)]
    inputs: BTreeMap<String, String>,
    #[serde(default)]
    mode: Option<String>,
    #[serde(default)]
    target: Option<String>,
}

/// Permission mode must be one of the three when present.
fn validate_mode(mode: Option<&str>) -> Result<(), ApiError> {
    match mode {
        None | Some("ask") | Some("bypass") | Some("readonly") => Ok(()),
        Some(other) => Err(ApiError::bad_request(format!(
            "invalid mode {other:?} (expected ask|bypass|readonly)"
        ))),
    }
}

/// Build the LaunchRequest and delegate to the port. Pure of AppState so it's
/// unit-testable with a mock launcher.
async fn dispatch_with_launcher(
    name: &str,
    body: DispatchBody,
    launcher: Arc<dyn RunLauncher>,
) -> Result<String, ApiError> {
    let req = LaunchRequest {
        workflow: name.to_string(),
        inputs: body.inputs,
        mode: body.mode,
        target: body.target,
    };
    launcher.launch(req).await.map_err(|e| match e {
        LaunchError::Invalid(m) => ApiError::bad_request(m),
        LaunchError::Spawn(m) => ApiError::internal(m),
    })
}

/// `POST /api/workflows/:name/dispatch` — start a fresh run of this workflow via
/// the installed RunLauncher (the `cp serve` subprocess adapter). Read-only
/// `rupu cp` (no launcher) returns 501.
async fn dispatch_workflow(
    State(s): State<AppState>,
    Path(name): Path<String>,
    body: Option<Json<DispatchBody>>,
) -> ApiResult<Json<serde_json::Value>> {
    let launcher = s
        .launcher
        .clone()
        .ok_or_else(|| ApiError::not_available("run dispatch requires `rupu cp serve`"))?;

    // Validate the workflow exists (stem under <global>/workflows/).
    let path = s.global_dir.join("workflows").join(format!("{name}.yaml"));
    if !path.exists() {
        return Err(ApiError::not_found(format!("workflow {name} not found")));
    }

    let body = body.map(|b| b.0).unwrap_or(DispatchBody {
        inputs: BTreeMap::new(),
        mode: None,
        target: None,
    });
    validate_mode(body.mode.as_deref())?;

    let run_id = dispatch_with_launcher(&name, body, launcher).await?;
    Ok(Json(serde_json::json!({ "run_id": run_id })))
}
```

(Confirm `ApiError`/`ApiResult` and `AppState` are already imported at the top of
`workflows.rs`; add `Json` to the `axum` import if not present.)

- [ ] **Step 4: Run test + clippy**

Run: `cargo test -p rupu-cp --lib api::workflows` → PASS.
Run: `cargo clippy -p rupu-cp --all-targets` → clean.
Run: `rustfmt --edition 2021 --check crates/rupu-cp/src/api/workflows.rs` (format the one file if it reports diffs).

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-cp/src/api/workflows.rs
git commit -m "feat(cp): POST /api/workflows/:name/dispatch via RunLauncher port"
```

---

## Task A3: Frontend api method

**Files:**
- Modify: `crates/rupu-cp/web/src/lib/api.ts`

**Interfaces:**
- Produces: `api.dispatchWorkflow(name, body): Promise<{ run_id: string }>` and a
  `DispatchBody` type. (`api.cancelRun` already exists.)

- [ ] **Step 1: Add the type + method**

Add the type near the workflow types:

```ts
export interface DispatchBody {
  inputs: Record<string, string>;
  mode?: 'ask' | 'bypass' | 'readonly';
  target?: string;
}
```

Add the method inside the `api` object (near `getWorkflow`):

```ts
  dispatchWorkflow(name: string, body: DispatchBody): Promise<{ run_id: string }> {
    return request<{ run_id: string }>(
      `/api/workflows/${encodeURIComponent(name)}/dispatch`,
      { method: 'POST', body: JSON.stringify(body) },
    );
  },
```

- [ ] **Step 2: Typecheck**

Run: `cd crates/rupu-cp/web && npx tsc --noEmit` → clean.

- [ ] **Step 3: Commit**

```bash
git add crates/rupu-cp/web/src/lib/api.ts
git commit -m "feat(cp/web): api.dispatchWorkflow"
```

---

## Task A4: Run button + Run modal on WorkflowDetail

**Files:**
- Create: `crates/rupu-cp/web/src/components/RunWorkflowModal.tsx`
- Create: `crates/rupu-cp/web/src/components/RunWorkflowModal.test.tsx`
- Modify: `crates/rupu-cp/web/src/pages/WorkflowDetail.tsx`

**Interfaces:**
- Consumes: `api.dispatchWorkflow`, `DispatchBody`, `useNavigate`.
- Produces: `<RunWorkflowModal name inputDefs onClose />` where
  `inputDefs: string[]` are the workflow's declared input names. Exposes a pure
  helper `buildDispatchBody(values, mode, target): DispatchBody` for testing.

- [ ] **Step 1: Write the failing test**

`RunWorkflowModal.test.tsx`:

```tsx
import { describe, it, expect } from 'vitest';
import { buildDispatchBody } from './RunWorkflowModal';

describe('buildDispatchBody', () => {
  it('includes only non-empty inputs and trims target', () => {
    const body = buildDispatchBody({ a: '1', b: '' }, 'bypass', '  github:o/r  ');
    expect(body).toEqual({ inputs: { a: '1' }, mode: 'bypass', target: 'github:o/r' });
  });
  it('omits target when blank', () => {
    const body = buildDispatchBody({}, 'ask', '   ');
    expect(body).toEqual({ inputs: {}, mode: 'ask' });
  });
});
```

- [ ] **Step 2: Run to verify it fails**

Run: `cd crates/rupu-cp/web && npx vitest run src/components/RunWorkflowModal.test.tsx`
Expected: FAIL — module not found.

- [ ] **Step 3: Implement the modal**

`RunWorkflowModal.tsx`:

```tsx
// Lightweight Run launcher modal for a workflow. Collects declared inputs, a
// permission mode, and an optional run-target string, then POSTs the dispatch
// and navigates to the new run. Runs execute in the background (cp serve) and
// keep running even if this page or the CP closes.
import { useState } from 'react';
import { useNavigate } from 'react-router-dom';
import { api, type DispatchBody } from '../lib/api';

export function buildDispatchBody(
  values: Record<string, string>,
  mode: 'ask' | 'bypass' | 'readonly',
  target: string,
): DispatchBody {
  const inputs: Record<string, string> = {};
  for (const [k, v] of Object.entries(values)) {
    if (v !== '') inputs[k] = v;
  }
  const body: DispatchBody = { inputs, mode };
  const t = target.trim();
  if (t) body.target = t;
  return body;
}

export default function RunWorkflowModal({
  name,
  inputDefs,
  onClose,
}: {
  name: string;
  inputDefs: string[];
  onClose: () => void;
}) {
  const navigate = useNavigate();
  const [values, setValues] = useState<Record<string, string>>({});
  const [mode, setMode] = useState<'ask' | 'bypass' | 'readonly'>('ask');
  const [target, setTarget] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function submit() {
    setBusy(true);
    setError(null);
    try {
      const { run_id } = await api.dispatchWorkflow(name, buildDispatchBody(values, mode, target));
      navigate(`/runs/${encodeURIComponent(run_id)}`);
    } catch (e: unknown) {
      setError(e instanceof Error ? e.message : 'Failed to start run');
      setBusy(false);
    }
  }

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/30 p-4"
      onClick={onClose}
    >
      <div
        className="w-full max-w-lg rounded-xl border border-border bg-panel p-5 shadow-card"
        onClick={(e) => e.stopPropagation()}
      >
        <h2 className="text-lg font-semibold text-ink">Run {name}</h2>
        <p className="mt-1 text-xs text-ink-dim">
          Runs start in the background and keep running even if this page or the CP is closed.
        </p>

        {inputDefs.length > 0 && (
          <div className="mt-4 space-y-2">
            {inputDefs.map((k) => (
              <label key={k} className="block">
                <span className="text-[11px] text-ink-mute font-mono">{k}</span>
                <input
                  value={values[k] ?? ''}
                  onChange={(e) => setValues((v) => ({ ...v, [k]: e.target.value }))}
                  className="mt-0.5 w-full rounded-md border border-border bg-white px-2 py-1 text-sm text-ink"
                />
              </label>
            ))}
          </div>
        )}

        <div className="mt-4 flex flex-wrap gap-3">
          <label className="flex flex-col gap-1">
            <span className="text-[11px] text-ink-mute">Mode</span>
            <select
              value={mode}
              onChange={(e) => setMode(e.target.value as 'ask' | 'bypass' | 'readonly')}
              className="rounded-md border border-border bg-panel px-2 py-1 text-sm text-ink"
            >
              <option value="ask">ask</option>
              <option value="bypass">bypass</option>
              <option value="readonly">readonly</option>
            </select>
          </label>
          <label className="flex flex-1 flex-col gap-1 min-w-[12rem]">
            <span className="text-[11px] text-ink-mute">Target (optional)</span>
            <input
              value={target}
              onChange={(e) => setTarget(e.target.value)}
              placeholder="github:owner/repo (blank = cp serve dir)"
              className="rounded-md border border-border bg-white px-2 py-1 text-sm text-ink"
            />
          </label>
        </div>

        {error && <p className="mt-3 text-sm text-red-700">{error}</p>}

        <div className="mt-5 flex justify-end gap-2">
          <button
            onClick={onClose}
            className="rounded-md border border-border px-3 py-1.5 text-sm text-ink-dim hover:bg-slate-100"
          >
            Cancel
          </button>
          <button
            onClick={submit}
            disabled={busy}
            className="rounded-md bg-brand-600 px-3 py-1.5 text-sm font-medium text-white hover:bg-brand-500 disabled:opacity-60"
          >
            {busy ? 'Starting…' : 'Run'}
          </button>
        </div>
      </div>
    </div>
  );
}
```

- [ ] **Step 4: Run the test (green)**

Run: `cd crates/rupu-cp/web && npx vitest run src/components/RunWorkflowModal.test.tsx`
Expected: PASS.

- [ ] **Step 5: Wire the Run button into WorkflowDetail**

In `crates/rupu-cp/web/src/pages/WorkflowDetail.tsx`:
- Import: `import { useState } from 'react';` (already imported `useEffect, useState` — confirm), and
  `import RunWorkflowModal from '../components/RunWorkflowModal';`.
- Add modal state in the component: `const [runOpen, setRunOpen] = useState(false);`.
- Derive declared input names defensively from the parsed workflow:

```tsx
  const inputDefs =
    detail && typeof detail.workflow.inputs === 'object' && detail.workflow.inputs !== null
      ? Object.keys(detail.workflow.inputs as Record<string, unknown>)
      : [];
```

- In the header (next to the `<h1>` / scope chips), add a Run button:

```tsx
          <button
            onClick={() => setRunOpen(true)}
            className="ml-auto rounded-md bg-brand-600 px-3 py-1.5 text-sm font-medium text-white hover:bg-brand-500"
          >
            Run
          </button>
```

- Near the end of the returned JSX (inside the page root), render the modal:

```tsx
      {runOpen && (
        <RunWorkflowModal
          name={wfName}
          inputDefs={inputDefs}
          onClose={() => setRunOpen(false)}
        />
      )}
```

(`wfName` is the existing derived workflow name in WorkflowDetail. Ensure the
header flex container allows `ml-auto` to push the button right; if the header
isn't a flex row, wrap the title row in `flex items-center gap-2`.)

- [ ] **Step 6: Typecheck + build + full web tests**

Run: `cd crates/rupu-cp/web && npx tsc --noEmit` → clean.
Run: `cd crates/rupu-cp/web && npx vitest run` → all pass.
Run: `cd crates/rupu-cp/web && npm run build` → success.

- [ ] **Step 7: Commit**

```bash
git add crates/rupu-cp/web/src/components/RunWorkflowModal.tsx crates/rupu-cp/web/src/components/RunWorkflowModal.test.tsx crates/rupu-cp/web/src/pages/WorkflowDetail.tsx
git commit -m "feat(cp/web): Run button + launcher modal on WorkflowDetail"
```

---

## Task A5: Verify + PR

- [ ] **Step 1: Backend**

Run: `cargo test -p rupu-cp -p rupu-cli --lib 2>&1 | grep "test result"` → all ok.
Run: `cargo clippy -p rupu-cp --all-targets` → clean.

- [ ] **Step 2: Frontend**

Run: `cd crates/rupu-cp/web && npx tsc --noEmit && npx vitest run && npm run build` → all green.

- [ ] **Step 3: Manual smoke (recommended)**

`make cp-web && rupu cp serve`, open a workflow → Run → fill mode/target → it
navigates to the new run; verify the run keeps running after quitting `cp serve`
(it's a detached process); Cancel from the run page stops it.

- [ ] **Step 4: PR**

```bash
gh pr create --title "feat(cp): Run a workflow from the web UI (sub-project A)" --body "…"
```

---

## Self-review notes (author)
- Spec coverage: A1 detach (spec A1), A2 endpoint (spec A2), A3 api, A4 form
  (spec Frontend). Cancel was already complete (backend + RunDetail UI) — no task.
- Type consistency: handler builds `LaunchRequest` exactly as defined in
  `launcher.rs`; `DispatchBody` (TS) mirrors the Rust body; `buildDispatchBody`
  output matches `DispatchBody`.
- B (smart pickers: `/api/fs/browse` + `/api/repos` + working-dir on
  `LaunchRequest`) and C (agent run) are separate plans.
