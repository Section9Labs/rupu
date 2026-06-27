# CP Web Run — PR1: repository fuzzy-complete

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development. Steps use `- [ ]`.

**Goal:** Add a repository fuzzy-complete to the existing `LauncherSheet` Target field, backed by a new `/api/repos` that lists the logged-in SCM repos.

**Architecture:** Follow the existing `RunLauncher` port pattern: `rupu-cp` defines a `RepoLister` trait + `GET /api/repos` handler; `rupu-cli`'s `cp serve` installs an adapter that builds the SCM registry (same as `rupu repos list`) and lists repos. Read-only `rupu cp` (no adapter) → 501, frontend treats as empty.

**Tech Stack:** Rust + axum (rupu-cp), Rust + rupu-scm/rupu-auth (rupu-cli), React 18 + TS + Vitest + Tailwind (web).

Spec: `docs/superpowers/specs/2026-06-26-rupu-cp-web-run-design.md` → "PR1 — Repository fuzzy-complete".

## Global Constraints
- `rupu-cp` must NOT gain `rupu-scm`/`rupu-auth` deps — the registry lives in the `rupu-cli` adapter behind the port (mirror `crates/rupu-cp/src/launcher.rs` + `crates/rupu-cli/src/cp_launcher.rs`).
- `#![deny(clippy::all)]`; per-file `rustfmt` only (never package-wide).
- Web: pure-logic tests node env; component tests `// @vitest-environment jsdom` + `afterEach(cleanup)` (vitest `globals: false`).
- Reuse, do not redefine: `ApiError::{not_available, internal}`, `AppState`, the `async_trait` crate (already a dep where ports live).

---

## Task 1: `RepoLister` port (`rupu-cp`)

**Files:** Create `crates/rupu-cp/src/repos.rs`; modify `crates/rupu-cp/src/lib.rs`, `crates/rupu-cp/src/state.rs`.

**Interfaces (produces):** `RepoEntry { platform, repo, default_branch, private }`, `RepoListError`, `trait RepoLister { async fn list(&self) -> Result<Vec<RepoEntry>, RepoListError> }`, `AppState.repos: Option<Arc<dyn RepoLister>>`, `AppState::with_repos(...)`.

- [ ] **Step 1: Create the port** — `crates/rupu-cp/src/repos.rs`:

```rust
//! `RepoLister` port — lists repos from the logged-in SCM accounts. rupu-cp
//! defines it; rupu-cli's `cp serve` provides the registry-backed adapter.
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct RepoEntry {
    /// Platform id, e.g. "github" | "gitlab".
    pub platform: String,
    /// "owner/name".
    pub repo: String,
    pub default_branch: String,
    pub private: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum RepoListError {
    #[error("failed to list repos: {0}")]
    Backend(String),
}

#[async_trait::async_trait]
pub trait RepoLister: Send + Sync {
    async fn list(&self) -> Result<Vec<RepoEntry>, RepoListError>;
}
```

- [ ] **Step 2:** In `crates/rupu-cp/src/lib.rs` add `pub mod repos;` next to `pub mod launcher;`. In the lib's options/builder where `with_launcher` is wired (look near `pub launcher:` in lib.rs ~line 32 and the `.with_launcher(opts.launcher)` call ~line 91), add a parallel `repos` option field + `.with_repos(opts.repos)`. Mirror the launcher field exactly.

- [ ] **Step 3:** In `crates/rupu-cp/src/state.rs` add to `AppState`:
```rust
    /// Optional repo-lister port; rupu-cli's `cp serve` installs the adapter.
    pub repos: Option<Arc<dyn crate::repos::RepoLister>>,
```
initialize `repos: None` in the constructor, and add:
```rust
    pub fn with_repos(mut self, repos: Option<Arc<dyn crate::repos::RepoLister>>) -> Self {
        self.repos = repos;
        self
    }
```
(mirror `with_launcher` exactly, including any `Arc` import already present.)

- [ ] **Step 4:** `cargo build -p rupu-cp` → compiles.

- [ ] **Step 5: Commit** — `git add -A && git commit -m "feat(cp): RepoLister port + AppState.repos"`

---

## Task 2: `GET /api/repos` handler (`rupu-cp`)

**Files:** Create `crates/rupu-cp/src/api/repos.rs`; modify `crates/rupu-cp/src/api/mod.rs` (or wherever api submodules are declared) and the server router (`crates/rupu-cp/src/server.rs` — where other `crate::api::*::routes()` are `.merge`d).

**Interfaces (produces):** `GET /api/repos -> Vec<RepoEntry>`; 501 when `repos` port is `None`.

- [ ] **Step 1: Write the failing test** — at the bottom of `crates/rupu-cp/src/api/repos.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::repos::{RepoEntry, RepoLister, RepoListError};
    use std::sync::Arc;

    struct MockRepos(Vec<RepoEntry>);
    #[async_trait::async_trait]
    impl RepoLister for MockRepos {
        async fn list(&self) -> Result<Vec<RepoEntry>, RepoListError> {
            Ok(self.0.clone())
        }
    }

    #[tokio::test]
    async fn lists_from_port() {
        let entry = RepoEntry {
            platform: "github".into(),
            repo: "o/r".into(),
            default_branch: "main".into(),
            private: false,
        };
        let port: Arc<dyn RepoLister> = Arc::new(MockRepos(vec![entry]));
        let out = list_repos_with(Some(port)).await.expect("ok");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].repo, "o/r");
    }

    #[tokio::test]
    async fn missing_port_is_not_available() {
        let err = list_repos_with(None).await.expect_err("no port");
        assert_eq!(err.0, axum::http::StatusCode::NOT_IMPLEMENTED);
    }
}
```

- [ ] **Step 2:** `cargo test -p rupu-cp --lib api::repos` → FAILS (symbols missing).

- [ ] **Step 3: Implement** `crates/rupu-cp/src/api/repos.rs`:

```rust
use crate::{
    error::{ApiError, ApiResult},
    repos::{RepoEntry, RepoLister},
    state::AppState,
};
use axum::{routing::get, Json, Router};
use std::sync::Arc;

pub fn routes() -> Router<AppState> {
    Router::new().route("/api/repos", get(list_repos))
}

/// Core, testable without axum State: returns the port's entries or 501.
async fn list_repos_with(port: Option<Arc<dyn RepoLister>>) -> ApiResult<Vec<RepoEntry>> {
    let port = port.ok_or_else(|| {
        ApiError::not_available("repo listing requires `rupu cp serve` with SCM credentials")
    })?;
    port.list()
        .await
        .map_err(|e| ApiError::internal(e.to_string()))
}

async fn list_repos(
    axum::extract::State(s): axum::extract::State<AppState>,
) -> ApiResult<Json<Vec<RepoEntry>>> {
    Ok(Json(list_repos_with(s.repos.clone()).await?))
}
```

- [ ] **Step 4:** Declare the module (add `pub mod repos;` to `crates/rupu-cp/src/api/mod.rs` beside the other api modules) and merge the routes in `server.rs` (add `.merge(crate::api::repos::routes())` next to the other merges).

- [ ] **Step 5:** `cargo test -p rupu-cp --lib api::repos` → PASS; `cargo clippy -p rupu-cp --all-targets` → clean; `rustfmt --edition 2021` the two changed files if `--check` flags them.

- [ ] **Step 6: Commit** — `git add -A && git commit -m "feat(cp): GET /api/repos via RepoLister port"`

---

## Task 3: `cp serve` repo adapter (`rupu-cli`)

**Files:** Create `crates/rupu-cli/src/cp_repos.rs`; modify `crates/rupu-cli/src/lib.rs` (add `pub mod cp_repos;`) and `crates/rupu-cli/src/cmd/cp.rs` (install the adapter in `serve`).

**Interfaces (consumes):** `rupu_cp::repos::{RepoEntry, RepoLister, RepoListError}`; mirrors `rupu repos list` (`crates/rupu-cli/src/cmd/repos.rs::list_inner`).

- [ ] **Step 1: Write the failing test** — mapping is the unit worth testing. In `crates/rupu-cli/src/cp_repos.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::to_entry;
    use rupu_scm::{Platform, RepoRef};

    #[test]
    fn maps_repo_to_entry() {
        let repo = rupu_scm::Repo {
            r: RepoRef { platform: Platform::Github, owner: "o".into(), repo: "r".into() },
            default_branch: "main".into(),
            clone_url_https: String::new(),
            clone_url_ssh: String::new(),
            private: true,
            description: None,
        };
        let e = to_entry(Platform::Github, &repo);
        assert_eq!(e.platform, "github");
        assert_eq!(e.repo, "o/r");
        assert_eq!(e.default_branch, "main");
        assert!(e.private);
    }
}
```

(Confirm `RepoRef`/`Repo` field names against `crates/rupu-scm/src/types.rs` before writing — adjust the literal if a field differs, e.g. extra fields.)

- [ ] **Step 2:** `cargo test -p rupu-cli --lib cp_repos` → FAILS.

- [ ] **Step 3: Implement** `crates/rupu-cli/src/cp_repos.rs`:

```rust
//! `cp serve` adapter for rupu-cp's `RepoLister` port. Lists repos across the
//! logged-in platforms via the SCM registry (same path as `rupu repos list`).
use rupu_cp::repos::{RepoEntry, RepoLister, RepoListError};
use rupu_scm::{Platform, Registry};
use std::sync::Arc;

pub struct CpRepoLister {
    pub registry: Arc<Registry>,
}

pub(crate) fn to_entry(p: Platform, r: &rupu_scm::Repo) -> RepoEntry {
    RepoEntry {
        platform: p.to_string(),
        repo: format!("{}/{}", r.r.owner, r.r.repo),
        default_branch: r.default_branch.clone(),
        private: r.private,
    }
}

#[async_trait::async_trait]
impl RepoLister for CpRepoLister {
    async fn list(&self) -> Result<Vec<RepoEntry>, RepoListError> {
        let mut out = Vec::new();
        for p in [Platform::Github, Platform::Gitlab] {
            let Some(conn) = self.registry.repo(p) else {
                continue;
            };
            match conn.list_repos().await {
                Ok(repos) => out.extend(repos.iter().map(|r| to_entry(p, r))),
                Err(e) => tracing::warn!(platform = %p, error = %e, "list_repos failed; skipping"),
            }
        }
        Ok(out)
    }
}
```

- [ ] **Step 4: Install in `cmd/cp.rs` `serve`.** Near where `SubprocessLauncher` is built and `state.with_launcher(...)` is called: build a registry once and install the repo lister. Mirror `rupu repos list` (`cmd/repos.rs::list_inner`) for the registry build:

```rust
    // Repo lister for the web Run target picker.
    let repos: Option<Arc<dyn rupu_cp::repos::RepoLister>> = {
        let resolver = rupu_auth::KeychainResolver::new();
        let global_cfg = global_dir.join("config.toml");
        let cfg = rupu_config::layer_files(Some(&global_cfg), None).unwrap_or_default();
        let registry = Arc::new(rupu_scm::Registry::discover(&resolver, &cfg).await);
        Some(Arc::new(crate::cp_repos::CpRepoLister { registry }))
    };
```
and add `.with_repos(repos)` to the `AppState` builder chain (next to `.with_launcher(...)`). Adjust names to the actual locals in `cp.rs` (`global_dir` exists there). If `layer_files` has no `unwrap_or_default`, handle the Result with a `match`/`?` as the surrounding code does.

- [ ] **Step 5:** `cargo test -p rupu-cli --lib cp_repos` → PASS; `cargo build -p rupu-cli` → compiles; clippy on the new file clean.

- [ ] **Step 6: Commit** — `git add -A && git commit -m "feat(cp): cp serve repo-lister adapter (SCM registry)"`

---

## Task 4: Frontend api (`web`)

**Files:** Modify `crates/rupu-cp/web/src/lib/api.ts`.

- [ ] **Step 1:** Add the type + method:

```ts
export interface RepoEntry {
  platform: string;
  repo: string; // owner/name
  default_branch: string;
  private: boolean;
}
```
```ts
  // Returns [] when no launcher/repo adapter is installed (read-only cp → 501).
  async getRepos(): Promise<RepoEntry[]> {
    try {
      return await request<RepoEntry[]>('/api/repos');
    } catch (e) {
      if (e instanceof ApiError && e.status === 501) return [];
      throw e;
    }
  },
```
(Confirm `ApiError` with a `.status` field is the one exported in api.ts — RunDetail uses `e.status === 404`, so it exists.)

- [ ] **Step 2:** `cd crates/rupu-cp/web && npx tsc --noEmit` → clean.
- [ ] **Step 3: Commit** — `git add -A && git commit -m "feat(cp/web): api.getRepos"`

---

## Task 5: `Combobox` component (`web`)

**Files:** Create `crates/rupu-cp/web/src/components/Combobox.tsx` and `Combobox.test.tsx`.

**Interfaces (produces):** `<Combobox value onChange options placeholder />` where `options: string[]`; free typing allowed; a filtered dropdown (substring, case-insensitive) with ↑/↓/Enter/Esc; selecting sets `value`. Exposes pure `filterOptions(options, query): string[]`.

- [ ] **Step 1: Failing test** — `Combobox.test.tsx`:

```tsx
import { describe, it, expect } from 'vitest';
import { filterOptions } from './Combobox';

describe('filterOptions', () => {
  it('substring, case-insensitive, empty query returns all', () => {
    const opts = ['github:o/api', 'github:o/web', 'gitlab:g/svc'];
    expect(filterOptions(opts, '')).toEqual(opts);
    expect(filterOptions(opts, 'API')).toEqual(['github:o/api']);
    expect(filterOptions(opts, 'git')).toEqual(opts);
  });
});
```

- [ ] **Step 2:** `cd crates/rupu-cp/web && npx vitest run src/components/Combobox.test.tsx` → FAILS.

- [ ] **Step 3: Implement** `Combobox.tsx`:

```tsx
// Minimal typeahead: a text input + filtered suggestion listbox. Free typing is
// always allowed (value is the raw text); picking a suggestion sets the value.
import { useState } from 'react';

export function filterOptions(options: string[], query: string): string[] {
  const q = query.trim().toLowerCase();
  if (!q) return options;
  return options.filter((o) => o.toLowerCase().includes(q));
}

export default function Combobox({
  value,
  onChange,
  options,
  placeholder,
  disabled,
  className,
}: {
  value: string;
  onChange: (v: string) => void;
  options: string[];
  placeholder?: string;
  disabled?: boolean;
  className?: string;
}) {
  const [open, setOpen] = useState(false);
  const [active, setActive] = useState(0);
  const matches = filterOptions(options, value).slice(0, 50);

  function choose(v: string) {
    onChange(v);
    setOpen(false);
  }

  return (
    <div className="relative">
      <input
        type="text"
        value={value}
        disabled={disabled}
        placeholder={placeholder}
        className={className}
        onChange={(e) => {
          onChange(e.target.value);
          setOpen(true);
          setActive(0);
        }}
        onFocus={() => setOpen(true)}
        onBlur={() => setTimeout(() => setOpen(false), 120)}
        onKeyDown={(e) => {
          if (!open || matches.length === 0) return;
          if (e.key === 'ArrowDown') {
            e.preventDefault();
            setActive((a) => Math.min(a + 1, matches.length - 1));
          } else if (e.key === 'ArrowUp') {
            e.preventDefault();
            setActive((a) => Math.max(a - 1, 0));
          } else if (e.key === 'Enter') {
            e.preventDefault();
            choose(matches[active]);
          } else if (e.key === 'Escape') {
            setOpen(false);
          }
        }}
      />
      {open && matches.length > 0 && (
        <ul className="absolute z-10 mt-1 max-h-56 w-full overflow-auto rounded-md border border-border bg-white shadow-card">
          {matches.map((o, i) => (
            <li key={o}>
              <button
                type="button"
                onMouseDown={(e) => {
                  e.preventDefault();
                  choose(o);
                }}
                className={
                  'block w-full px-2.5 py-1.5 text-left text-[13px] font-mono ' +
                  (i === active ? 'bg-brand-50 text-brand-700' : 'text-ink hover:bg-slate-50')
                }
              >
                {o}
              </button>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
```

- [ ] **Step 4:** `npx vitest run src/components/Combobox.test.tsx` → PASS.
- [ ] **Step 5: Commit** — `git add -A && git commit -m "feat(cp/web): Combobox typeahead component"`

---

## Task 6: Wire repo combobox into `LauncherSheet` (`web`)

**Files:** Modify `crates/rupu-cp/web/src/components/LauncherSheet.tsx`.

- [ ] **Step 1:** Fetch repos when the sheet opens, build `platform:repo` options, and replace the plain Target `<input>` with `<Combobox>`. Add near the other state:

```tsx
import Combobox from './Combobox';
import { api, type LaunchMode, type RepoEntry } from '../lib/api';
```
```tsx
  const [repoOptions, setRepoOptions] = useState<string[]>([]);
  useEffect(() => {
    let cancelled = false;
    api
      .getRepos()
      .then((rs: RepoEntry[]) => {
        if (!cancelled) setRepoOptions(rs.map((r) => `${r.platform}:${r.repo}`));
      })
      .catch(() => {
        /* no adapter / offline → free-text only */
      });
    return () => {
      cancelled = true;
    };
  }, []);
```

Replace the Target `<input ... />` (the one with `value={target}`) with:

```tsx
            <Combobox
              value={target}
              onChange={setTarget}
              options={repoOptions}
              disabled={launching}
              placeholder="e.g. github:owner/repo"
              className={fieldCls}
            />
```
Keep the helper text ("leave blank to run in this workspace"). `target` still flows to `api.launchRun({ target: target.trim() || undefined })` unchanged — free text (PR/issue refs, blank) still works.

- [ ] **Step 2:** `cd crates/rupu-cp/web && npx tsc --noEmit && npx vitest run && npm run build` → all green.
- [ ] **Step 3: Commit** — `git add -A && git commit -m "feat(cp/web): repo fuzzy-complete in LauncherSheet target"`

---

## Task 7: Verify + PR

- [ ] `cargo test -p rupu-cp -p rupu-cli --lib 2>&1 | grep "test result"` → ok; `cargo clippy -p rupu-cp --all-targets` → clean.
- [ ] `cd crates/rupu-cp/web && npx tsc --noEmit && npx vitest run && npm run build` → green.
- [ ] Manual: `make cp-web && rupu cp serve`, open a workflow → Run → Target shows repo suggestions as you type; picking one fills `platform:owner/repo`; free text still works.
- [ ] `gh pr create --title "feat(cp): repository fuzzy-complete in the Run target (+ detached runs)" --body "…"` (the branch already contains the detach-runs commit `b2dc844`).

## Self-review notes
- Spec PR1 coverage: port (T1), endpoint (T2), adapter (T3), api (T4), combobox (T5), wiring (T6). Detach fix already on branch.
- The CP gains no `rupu-scm`/`rupu-auth` dep — registry stays in the rupu-cli adapter behind the port.
- No cache in v1 (frontend fetches once per sheet-open); note as future if slow.
- Type parity: `RepoEntry` (Rust Serialize) ↔ `RepoEntry` (TS); combobox value is the raw target string `platform:owner/repo`.
