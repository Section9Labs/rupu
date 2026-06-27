# CP Web Run — PR2: directory picker (browse + fuzzy)

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development. Steps use `- [ ]`.

**Goal:** Let the Run modal target a working directory — chosen via a file browser (drill into subdirs) and/or fuzzy-complete from previous projects — by adding a `working_dir` to the launch path and a `/api/fs/browse` endpoint.

**Architecture:** `working_dir` flows `LaunchBody` → `LaunchRequest` → `SubprocessLauncher` (spawns the run with `current_dir`). A new `GET /api/fs/browse` lists subdirectories on the CP host (local tool; pure FS read, no port needed). The frontend gains a target-mode selector (Workspace / Directory / Repository) and a `DirectoryPicker`.

**Tech Stack:** Rust + axum (rupu-cp), Rust (rupu-cli), React 18 + TS + Vitest + Tailwind.

Spec: `docs/superpowers/specs/2026-06-26-rupu-cp-web-run-design.md` → "PR2 — Directory picker".

## Global Constraints
- `#![deny(clippy::all)]`; per-file `rustfmt` only.
- Web vitest `globals: false`: component tests need `// @vitest-environment jsdom` + `afterEach(cleanup)`; pure-logic tests run node env.
- `working_dir` and `target` are independent optional fields; the launcher already runs in cp-serve's cwd when neither is set. A repo `target` clones itself; a `working_dir` runs in place.
- Reuse existing `Combobox` (`components/Combobox.tsx`, `ComboboxOption {value,label}`, exported `filterOptions`).

---

## Task 1: `working_dir` through the launch path

**Files:** `crates/rupu-cp/src/launcher.rs`, `crates/rupu-cp/src/api/workflows.rs`, `crates/rupu-cli/src/cp_launcher.rs`.

**Interfaces (produces):** `LaunchRequest.working_dir: Option<String>`; `LaunchBody.working_dir`; the spawned child runs with `current_dir(working_dir)` when set.

- [ ] **Step 1: Failing test** — extend the existing `dispatch`/launch test in `crates/rupu-cp/src/api/workflows.rs` (the `MockLauncher` captures the `LaunchRequest`). Add a test that `launch_run` forwards `working_dir`:

```rust
    #[tokio::test]
    async fn launch_forwards_working_dir() {
        let mock = Arc::new(MockLauncher {
            last: Mutex::new(None),
            run_id: "run_X".into(),
        });
        let tmp = tempfile::tempdir().unwrap();
        let s = test_state(&tmp).with_launcher(Some(mock.clone()));
        // Use the existing test_state + the same workflow fixture the other
        // launch test uses (create the workflow file the same way).
        let body = LaunchBody {
            inputs: Default::default(),
            mode: None,
            target: None,
            working_dir: Some("/tmp/projX".into()),
        };
        let _ = launch_run(State(s), Path("nightly".into()), Some(Json(body))).await;
        let got = mock.last.lock().unwrap().clone().unwrap();
        assert_eq!(got.working_dir.as_deref(), Some("/tmp/projX"));
    }
```
(Mirror however the existing `launch_run_invokes_launcher_and_returns_run_id` test sets up `test_state` + the `nightly` workflow file — copy that setup. If the existing test constructs `LaunchBody { … }` literally, add `working_dir: None` there too so it still compiles.)

- [ ] **Step 2:** Run `cargo test -p rupu-cp --lib api::workflows` → FAILS (no `working_dir` field).

- [ ] **Step 3: Implement.**
- `crates/rupu-cp/src/launcher.rs` — add to `LaunchRequest`:
  ```rust
      /// Working directory for the run (project/dir target). When `None` the
      /// run executes in the cp-serve process's cwd.
      pub working_dir: Option<String>,
  ```
- `crates/rupu-cp/src/api/workflows.rs` — add `#[serde(default)] working_dir: Option<String>` to `LaunchBody`, and pass `working_dir: b.working_dir` when building `LaunchRequest` in `launch_run`. Update any `LaunchRequest {…}`/`LaunchBody {…}` literals in this file's tests to include the new field.
- `crates/rupu-cli/src/cp_launcher.rs` — in `launch`, after building `cmd`, set the working dir when present:
  ```rust
          if let Some(dir) = req.working_dir.as_deref() {
              cmd.current_dir(dir);
          }
  ```
  (`build_run_argv` is unchanged — `working_dir` is not a CLI arg; it's the child's cwd, which is how `rupu workflow run` discovers the project.)

- [ ] **Step 4:** `cargo test -p rupu-cp --lib api::workflows` → PASS. `cargo build -p rupu-cli` → compiles. `cargo clippy -p rupu-cp --all-targets` → clean. Also fix any other `LaunchRequest {…}` literal in `cp_launcher.rs` tests (the `build_run_argv` tests build a `LaunchRequest` — add `working_dir: None`).

- [ ] **Step 5: Commit** — `git add -A && git commit -m "feat(cp): working_dir on LaunchRequest (run in a chosen directory)"`

---

## Task 2: `GET /api/fs/browse`

**Files:** Create `crates/rupu-cp/src/api/fs.rs`; modify `crates/rupu-cp/src/api/mod.rs` + `server.rs`.

**Interfaces (produces):** `GET /api/fs/browse?path=<dir>` → `{ path: String, parent: Option<String>, dirs: Vec<{ name: String, path: String }> }`. No `path` → `$HOME`. Lists immediate subdirectories only, sorted by name; hidden dirs (leading `.`) excluded.

- [ ] **Step 1: Failing test** — in `crates/rupu-cp/src/api/fs.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_subdirs_sorted_excludes_hidden_and_files() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir(root.join("beta")).unwrap();
        std::fs::create_dir(root.join("alpha")).unwrap();
        std::fs::create_dir(root.join(".hidden")).unwrap();
        std::fs::write(root.join("file.txt"), b"x").unwrap();

        let out = browse_dir(root.to_str().unwrap()).expect("ok");
        assert_eq!(
            out.dirs.iter().map(|d| d.name.clone()).collect::<Vec<_>>(),
            vec!["alpha", "beta"]
        );
        assert_eq!(out.parent.as_deref(), root.parent().and_then(|p| p.to_str()));
    }

    #[test]
    fn missing_dir_errors() {
        assert!(browse_dir("/no/such/dir/xyz").is_err());
    }
}
```

- [ ] **Step 2:** `cargo test -p rupu-cp --lib api::fs` → FAILS.

- [ ] **Step 3: Implement** `crates/rupu-cp/src/api/fs.rs`:

```rust
use crate::{
    error::{ApiError, ApiResult},
    state::AppState,
};
use axum::{extract::Query, routing::get, Json, Router};
use serde::{Deserialize, Serialize};

pub fn routes() -> Router<AppState> {
    Router::new().route("/api/fs/browse", get(browse))
}

#[derive(Serialize)]
pub(crate) struct FsEntry {
    pub(crate) name: String,
    pub(crate) path: String,
}

#[derive(Serialize)]
pub(crate) struct BrowseResult {
    pub(crate) path: String,
    pub(crate) parent: Option<String>,
    pub(crate) dirs: Vec<FsEntry>,
}

#[derive(Deserialize)]
struct BrowseQuery {
    path: Option<String>,
}

/// List immediate subdirectories of `path` (sorted, hidden excluded). Pure +
/// testable. Errors when the path is missing/unreadable/not a directory.
pub(crate) fn browse_dir(path: &str) -> Result<BrowseResult, String> {
    let p = std::path::Path::new(path)
        .canonicalize()
        .map_err(|e| format!("{path}: {e}"))?;
    if !p.is_dir() {
        return Err(format!("{} is not a directory", p.display()));
    }
    let mut dirs: Vec<FsEntry> = std::fs::read_dir(&p)
        .map_err(|e| e.to_string())?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                return None;
            }
            Some(FsEntry {
                path: e.path().to_string_lossy().into_owned(),
                name,
            })
        })
        .collect();
    dirs.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(BrowseResult {
        path: p.to_string_lossy().into_owned(),
        parent: p.parent().map(|x| x.to_string_lossy().into_owned()),
        dirs,
    })
}

fn home_dir() -> String {
    std::env::var("HOME").unwrap_or_else(|_| "/".to_string())
}

async fn browse(Query(q): Query<BrowseQuery>) -> ApiResult<Json<BrowseResult>> {
    let path = q.path.filter(|s| !s.is_empty()).unwrap_or_else(home_dir);
    browse_dir(&path)
        .map(Json)
        .map_err(ApiError::bad_request)
}
```

- [ ] **Step 4:** declare `pub mod fs;` in `crates/rupu-cp/src/api/mod.rs`; `.merge(crate::api::fs::routes())` in `server.rs`. `cargo test -p rupu-cp --lib api::fs` → PASS; clippy clean.

- [ ] **Step 5: Commit** — `git add -A && git commit -m "feat(cp): GET /api/fs/browse (directory listing)"`

---

## Task 3: Frontend api (`web`)

**Files:** `crates/rupu-cp/web/src/lib/api.ts`.

- [ ] **Step 1:** Add types + methods, and add `working_dir` to `launchRun`:

```ts
export interface FsEntry { name: string; path: string; }
export interface BrowseResult { path: string; parent: string | null; dirs: FsEntry[]; }
```
```ts
  browseDir(path?: string): Promise<BrowseResult> {
    const qs = path ? `?path=${encodeURIComponent(path)}` : '';
    return request<BrowseResult>(`/api/fs/browse${qs}`);
  },
```
Change `launchRun` to accept + send `working_dir`:
```ts
  launchRun(
    workflow: string,
    opts: { inputs?: Record<string, string>; mode?: LaunchMode; target?: string; working_dir?: string } = {},
  ): Promise<LaunchResult> {
    return request<LaunchResult>(`/api/workflows/${encodeURIComponent(workflow)}/run`, {
      method: 'POST',
      body: JSON.stringify({ inputs: opts.inputs, mode: opts.mode, target: opts.target, working_dir: opts.working_dir }),
    });
  },
```

- [ ] **Step 2:** `cd crates/rupu-cp/web && npx tsc --noEmit` → clean.
- [ ] **Step 3: Commit** — `git add -A && git commit -m "feat(cp/web): api.browseDir + working_dir on launchRun"`

---

## Task 4: `DirectoryPicker` component (`web`)

**Files:** Create `crates/rupu-cp/web/src/components/DirectoryPicker.tsx` + `DirectoryPicker.test.tsx`.

**Interfaces (produces):** `<DirectoryPicker value onChange />` (value = selected absolute dir string). A file browser: shows the current dir's subdirs (drill in), a parent (`..`) entry, project quick-picks (from `getProjects()`), and a free-text path input with fuzzy-complete over project paths. Exposes pure `matchProjects(paths, query): string[]` for testing.

- [ ] **Step 1: Failing test** — `DirectoryPicker.test.tsx`:

```tsx
import { describe, it, expect } from 'vitest';
import { matchProjects } from './DirectoryPicker';

describe('matchProjects', () => {
  it('substring, case-insensitive; empty query returns all', () => {
    const paths = ['/Users/m/Code/api', '/Users/m/Code/web', '/tmp/scratch'];
    expect(matchProjects(paths, '')).toEqual(paths);
    expect(matchProjects(paths, 'CODE')).toEqual(['/Users/m/Code/api', '/Users/m/Code/web']);
    expect(matchProjects(paths, 'scr')).toEqual(['/tmp/scratch']);
  });
});
```

- [ ] **Step 2:** `cd crates/rupu-cp/web && npx vitest run src/components/DirectoryPicker.test.tsx` → FAILS.

- [ ] **Step 3: Implement** `DirectoryPicker.tsx`:

```tsx
// Directory picker: free-text path with fuzzy-complete over past projects, plus
// a browse list (drill into subdirs / go up). The chosen absolute path is the
// value (sent as the run's working_dir).
import { useEffect, useState } from 'react';
import { api, type FsEntry, type ProjectRow } from '../lib/api';

export function matchProjects(paths: string[], query: string): string[] {
  const q = query.trim().toLowerCase();
  if (!q) return paths;
  return paths.filter((p) => p.toLowerCase().includes(q));
}

export default function DirectoryPicker({
  value,
  onChange,
}: {
  value: string;
  onChange: (v: string) => void;
}) {
  const [projects, setProjects] = useState<string[]>([]);
  const [dirs, setDirs] = useState<FsEntry[]>([]);
  const [parent, setParent] = useState<string | null>(null);
  const [browsePath, setBrowsePath] = useState<string>('');
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    api
      .getProjects()
      .then((ps: ProjectRow[]) => setProjects(ps.map((p) => p.path)))
      .catch(() => setProjects([]));
  }, []);

  function load(path?: string) {
    api
      .browseDir(path)
      .then((r) => {
        setDirs(r.dirs);
        setParent(r.parent);
        setBrowsePath(r.path);
        setError(null);
      })
      .catch((e: unknown) => setError(e instanceof Error ? e.message : 'Cannot read directory'));
  }

  // Initial browse (home) when the picker mounts.
  useEffect(() => {
    load(value || undefined);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const fieldCls =
    'w-full rounded-md border border-border bg-white px-2.5 py-1.5 text-[13px] text-ink placeholder:text-ink-mute focus:border-brand-500 focus:outline-none';
  const projMatches = matchProjects(projects, value).slice(0, 6);

  return (
    <div className="space-y-2">
      <input
        type="text"
        value={value}
        onChange={(e) => onChange(e.target.value)}
        placeholder="/path/to/project"
        aria-label="Directory path"
        className={fieldCls}
      />

      {projMatches.length > 0 && (
        <div className="flex flex-wrap gap-1">
          {projMatches.map((p) => (
            <button
              key={p}
              type="button"
              onClick={() => {
                onChange(p);
                load(p);
              }}
              className="rounded border border-border bg-slate-50 px-1.5 py-0.5 text-[11px] font-mono text-ink-dim hover:bg-slate-100"
            >
              {p}
            </button>
          ))}
        </div>
      )}

      <div className="rounded-md border border-border bg-white">
        <div className="flex items-center justify-between border-b border-border px-2 py-1 text-[11px] text-ink-mute">
          <span className="truncate font-mono">{browsePath || '…'}</span>
          <button
            type="button"
            onClick={() => onChange(browsePath)}
            className="ml-2 shrink-0 font-medium text-brand-600 hover:text-brand-700"
          >
            use this
          </button>
        </div>
        <ul className="max-h-44 overflow-auto py-1">
          {parent && (
            <li>
              <button
                type="button"
                onClick={() => load(parent)}
                className="block w-full px-2 py-1 text-left text-[12px] font-mono text-ink-dim hover:bg-slate-50"
              >
                ../
              </button>
            </li>
          )}
          {dirs.map((d) => (
            <li key={d.path}>
              <button
                type="button"
                onClick={() => {
                  onChange(d.path);
                  load(d.path);
                }}
                className="block w-full px-2 py-1 text-left text-[12px] font-mono text-ink hover:bg-slate-50"
              >
                {d.name}/
              </button>
            </li>
          ))}
          {dirs.length === 0 && !parent && (
            <li className="px-2 py-1 text-[12px] text-ink-mute">no subdirectories</li>
          )}
        </ul>
      </div>
      {error && <p className="text-[12px] text-red-700">{error}</p>}
    </div>
  );
}
```

- [ ] **Step 4:** `cd crates/rupu-cp/web && npx vitest run src/components/DirectoryPicker.test.tsx` → PASS.
- [ ] **Step 5: Commit** — `git add -A && git commit -m "feat(cp/web): DirectoryPicker (browse + project fuzzy-complete)"`

---

## Task 5: Target-mode selector in `LauncherSheet`

**Files:** `crates/rupu-cp/web/src/components/LauncherSheet.tsx`.

**Interfaces (consumes):** `DirectoryPicker`, existing repo `Combobox`, `api.launchRun` (now with `working_dir`).

- [ ] **Step 1:** Add a target-mode state and render the matching control; pass the right field on launch.

Add imports + state:
```tsx
import DirectoryPicker from './DirectoryPicker';
type TargetMode = 'workspace' | 'directory' | 'repo';
```
```tsx
  const [targetMode, setTargetMode] = useState<TargetMode>('workspace');
  const [workingDir, setWorkingDir] = useState('');
```

Replace the current Target `<label>…<Combobox/>…</label>` block with a mode selector + conditional control:
```tsx
          <div>
            <span className="mb-1 block text-[12px] font-semibold uppercase tracking-wide text-ink-dim">
              Target
            </span>
            <div className="mb-2 flex gap-1">
              {(['workspace', 'directory', 'repo'] as TargetMode[]).map((m) => (
                <button
                  key={m}
                  type="button"
                  onClick={() => setTargetMode(m)}
                  disabled={launching}
                  className={
                    'rounded-md px-2 py-1 text-[12px] font-medium ' +
                    (targetMode === m
                      ? 'bg-brand-600 text-white'
                      : 'border border-border bg-white text-ink-dim hover:bg-slate-50')
                  }
                >
                  {m === 'workspace' ? 'This workspace' : m === 'directory' ? 'Directory' : 'Repository'}
                </button>
              ))}
            </div>
            {targetMode === 'workspace' && (
              <p className="text-[11px] text-ink-mute">Runs in the control-plane working directory.</p>
            )}
            {targetMode === 'directory' && (
              <DirectoryPicker value={workingDir} onChange={setWorkingDir} />
            )}
            {targetMode === 'repo' && (
              <Combobox
                value={target}
                onChange={setTarget}
                options={repoOptions}
                disabled={launching}
                aria-label="Target"
                placeholder="e.g. github:owner/repo"
                className={fieldCls}
              />
            )}
          </div>
```

Update `onLaunch` to send the field for the active mode:
```tsx
      const res = await api.launchRun(workflow, {
        inputs: Object.keys(inputs).length > 0 ? inputs : undefined,
        mode,
        target: targetMode === 'repo' ? target.trim() || undefined : undefined,
        working_dir: targetMode === 'directory' ? workingDir.trim() || undefined : undefined,
      });
```

(Keep everything else — inputs, mode, buttons — unchanged. Confirm `fieldCls`, `target`, `repoOptions`, `launching` are still in scope.)

- [ ] **Step 2:** `cd crates/rupu-cp/web && npx tsc --noEmit && npx vitest run && npm run build` → all green. (If an existing LauncherSheet test asserted on the old single Target input via `aria-label="Target"`, update it: the repo Combobox keeps `aria-label="Target"` only in repo mode — adjust the test to switch to repo mode first, or assert the mode selector. Keep tests green.)

- [ ] **Step 3: Commit** — `git add -A && git commit -m "feat(cp/web): target-mode selector (workspace/directory/repo) in LauncherSheet"`

---

## Task 6: Verify + PR

- [ ] `cargo test -p rupu-cp --lib 2>&1 | grep "test result"` → ok; `cargo clippy -p rupu-cp --all-targets` → clean; `cargo build -p rupu-cli` → ok. (rupu-cli's full `--lib` suite has a pre-existing unrelated session-test failure under local rustc 1.95 — confirm it's the same one, not new.)
- [ ] `cd crates/rupu-cp/web && npx tsc --noEmit && npx vitest run && npm run build` → green.
- [ ] Manual: `make cp-web && rupu cp serve`; open a workflow → Run → Directory tab: browse/drill into a dir or pick a project, Launch → run starts in that dir; Repository tab still fuzzy-completes.
- [ ] `gh pr create --title "feat(cp): directory browse + fuzzy-complete target for Run" --body "…"`

## Self-review notes
- Spec PR2 coverage: working_dir plumbing (T1), `/api/fs/browse` (T2), api (T3), DirectoryPicker browse+project-fuzzy (T4), target-mode wiring (T5).
- `working_dir`/`target` mutually exclusive by mode in the UI but independent on the wire; backend forwards whatever is set.
- `/api/fs/browse` reads the CP host FS — acceptable for a local personal tool; canonicalizes + errors on missing/again non-dir; hidden dirs excluded.
