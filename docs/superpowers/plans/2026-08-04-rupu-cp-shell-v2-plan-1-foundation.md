# rupu CP Shell v2 — Plan 1: Foundation & Shell

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship the `[ui.cp] shell` feature flag end-to-end and the complete v2 shell chrome (rail, top bar, routes, redirects, palette), with every existing capability reachable under the new 7-leaf IA via interim composite pages.

**Architecture:** The flag lives in `rupu-config` and reaches the browser as a `<meta name="rupu-shell">` tag injected by `rupu-cp`'s `embed.rs` at serve time (re-resolved from disk per index.html request — zero round-trips, hot-switchable). The web app branches once in `App.tsx`: `components/Layout.tsx` (v1, untouched) vs `components/v2/Shell.tsx`. v2 destination pages this plan are thin tabbed wrappers around the existing page components — full capability, new IA; later plans replace the bodies.

**Tech Stack:** Rust (axum, serde, rupu-config), React 18 + TypeScript + Vite + Tailwind, react-router-dom v6, vitest + @testing-library/react.

## Global Constraints

- No hardcoded hex colours anywhere — Tailwind token classes only (`bg-panel`, `text-ink-dim`, `border-border-strong`, …).
- `--c-ink-mute` is the dimmest tier allowed to carry text; `--c-border-strong` / `--c-surface-active` are fill/line tokens, never text colours.
- All animation must be `prefers-reduced-motion` guarded (reuse existing guarded classes; the `.sr-live`/`.sr-beacon` block already is).
- Do not touch `components/Layout.tsx` markup, `SidebarGroup`, the `rupu.sidebar.groups` localStorage key, or any v1 page test.
- Workspace deps only (root `Cargo.toml`); `#![deny(clippy::all)]`; no `unsafe`.
- **Never run package-wide `cargo fmt`** — `main` is fmt-dirty under the pinned toolchain; format per-file only.
- Detail routes are unchanged and must keep working under both shells: `/runs/:id`, `/transcript`, `/sessions/:id`, `/hosts/:id`, `/agents/:name`, `/workflows/:name`, `/coverage/:target/*`, `/projects/:wsId/*`, `/events`.
- Branch: `feat/cp-shell-v2-plan-1` off a **freshly fetched** `origin/main` (`git fetch origin main:refs/remotes/origin/main` first). All work lands via PR. Never `git stash pop` a shared stash.

Commands (run from repo root unless noted):
- Rust tests: `cargo test -p rupu-config` / `cargo test -p rupu-cp --test embed`
- Web tests: `cd crates/rupu-cp/web && npx vitest run <file>`
- Web build check: `cd crates/rupu-cp/web && npm run build`

---

### Task 1: `[ui.cp] shell` config field

**Files:**
- Modify: `crates/rupu-config/src/config.rs` (UiConfig at ~line 75; tests module at ~line 243)
- Modify: `crates/rupu-config/src/lib.rs` (re-export, ~line 34)
- Modify: `docs/configuration.md` (`[ui]` table, ~line 82)

**Interfaces:**
- Produces: `rupu_config::Config.ui.cp.shell: Option<String>` and re-exported `rupu_config::UiCpConfig`. Task 2 reads `resolved.config.ui.cp.shell`.

- [ ] **Step 1: Write the failing tests** — in the existing `mod tests` in `config.rs`:

```rust
#[test]
fn ui_cp_shell_parses() {
    let cfg: Config = toml::from_str("[ui.cp]\nshell = \"v2\"\n").unwrap();
    assert_eq!(cfg.ui.cp.shell.as_deref(), Some("v2"));
}

#[test]
fn ui_cp_defaults_to_absent() {
    let cfg: Config = toml::from_str("").unwrap();
    assert_eq!(cfg.ui.cp.shell, None);
}

#[test]
fn ui_cp_rejects_unknown_keys() {
    assert!(toml::from_str::<Config>("[ui.cp]\nshel = \"v2\"\n").is_err());
}
```

- [ ] **Step 2: Run to verify failure** — `cargo test -p rupu-config ui_cp` → FAIL (no field `cp` on `UiConfig`).

- [ ] **Step 3: Implement** — in `config.rs`, next to `UiSyntaxConfig` (same sibling-struct pattern as `[ui.syntax]`):

```rust
/// `[ui.cp]` — Control Plane web UI preferences.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct UiCpConfig {
    /// Web shell generation served by `rupu cp serve`: `"v1"` (default,
    /// classic sidebar shell) or `"v2"` (Shell v2 redesign). Any other
    /// value is treated as `"v1"` by the consumer.
    pub shell: Option<String>,
}
```

and on `UiConfig`:

```rust
    /// `[ui.cp]` — Control Plane web UI preferences.
    #[serde(default)]
    pub cp: UiCpConfig,
```

In `lib.rs`, extend the existing re-export line to include `UiCpConfig`.

- [ ] **Step 4: Run tests** — `cargo test -p rupu-config` → all PASS (including the pre-existing suite; `deny_unknown_fields` on `Config` means the new field must not break `layering.rs`/`parse.rs`).

- [ ] **Step 5: Document** — add to the `[ui]` table in `docs/configuration.md`:

```markdown
| `[ui.cp].shell`      | string | `v1`                 | `v1` \| `v2` — CP web shell generation (Shell v2 redesign). Requires rupu ≥ the version this lands in: older binaries reject unknown `[ui]` keys and silently fall back to a default config. |
```

- [ ] **Step 6: Commit** — `git add crates/rupu-config docs/configuration.md && git commit -m "feat(config): add [ui.cp] shell flag for CP shell v2"`

---

### Task 2: `embed.rs` meta-tag injection

**Files:**
- Modify: `crates/rupu-cp/src/embed.rs` (whole file, 34 lines)
- Test: `crates/rupu-cp/tests/embed.rs` (extend)

**Interfaces:**
- Consumes: `AppState.global_dir`, `rupu_config::resolve`, Task 1's `ui.cp.shell`.
- Produces: `index.html` responses carrying `<meta name="rupu-shell" content="v1"|"v2">`. Task 4's `lib/shell.ts` reads this tag.

- [ ] **Step 1: Write the failing integration tests** — in `tests/embed.rs`, add a config-aware spawner alongside the existing `spawn_server` (which stays untouched):

```rust
async fn spawn_server_with_config(config_toml: &str) -> SocketAddr {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("config.toml"), config_toml).unwrap();
    let state =
        rupu_cp::state::AppState::new(dir.path().into(), rupu_config::PricingConfig::default());
    std::mem::forget(dir);
    let app = rupu_cp::server::router(state, None);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr
}

#[tokio::test]
async fn index_carries_shell_meta_v1_by_default() {
    let addr = spawn_server().await;
    let body = reqwest::get(format!("http://{addr}/")).await.unwrap().text().await.unwrap();
    assert!(
        body.contains(r#"<meta name="rupu-shell" content="v1">"#),
        "expected v1 shell meta, got: {body}"
    );
}

#[tokio::test]
async fn index_carries_shell_meta_v2_when_configured() {
    let addr = spawn_server_with_config("[ui.cp]\nshell = \"v2\"\n").await;
    let body = reqwest::get(format!("http://{addr}/")).await.unwrap().text().await.unwrap();
    assert!(body.contains(r#"<meta name="rupu-shell" content="v2">"#));
}

#[tokio::test]
async fn spa_fallback_also_carries_shell_meta() {
    let addr = spawn_server_with_config("[ui.cp]\nshell = \"v2\"\n").await;
    let body = reqwest::get(format!("http://{addr}/overview"))
        .await.unwrap().text().await.unwrap();
    assert!(body.contains(r#"<meta name="rupu-shell" content="v2">"#));
}
```

- [ ] **Step 2: Run to verify failure** — `cargo test -p rupu-cp --test embed` → new tests FAIL (no meta in body).

- [ ] **Step 3: Implement** — rewrite `embed.rs` so both the exact `index.html` arm and the SPA-fallback arm inject; assets stream verbatim:

```rust
use crate::state::AppState;
use axum::extract::State;
use axum::http::{header, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "web/dist/"]
struct Assets;

/// Shell flag resolved fresh from the global config on every index.html
/// serve — `rupu config set ui.cp.shell v2` + a browser refresh switches
/// shells without restarting `cp serve` (same per-request-resolve contract
/// as `GET /api/config`).
fn resolve_shell(state: &AppState) -> &'static str {
    let global = state.global_dir.join("config.toml");
    let shell = rupu_config::resolve(Some(&global), None)
        .ok()
        .and_then(|r| r.config.ui.cp.shell);
    match shell.as_deref() {
        Some("v2") => "v2",
        _ => "v1",
    }
}

/// Insert `<meta name="rupu-shell" …>` after the first opening `<head>` tag.
/// Tolerates the build.rs placeholder index.html (has a `<head>`, no `#root`);
/// a document with no `<head>` is served unmodified.
fn inject_shell_meta(html: &str, shell: &str) -> String {
    match html.find("<head>") {
        Some(i) => {
            let at = i + "<head>".len();
            format!(
                "{}<meta name=\"rupu-shell\" content=\"{shell}\">{}",
                &html[..at],
                &html[at..]
            )
        }
        None => html.to_string(),
    }
}

fn serve_index(state: &AppState) -> Response {
    match Assets::get("index.html") {
        Some(content) => {
            let html = String::from_utf8_lossy(&content.data);
            let html = inject_shell_meta(&html, resolve_shell(state));
            ([(header::CONTENT_TYPE, "text/html")], html).into_response()
        }
        None => (StatusCode::NOT_FOUND, "web UI not embedded").into_response(),
    }
}

pub async fn static_handler(State(state): State<AppState>, uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    if path.is_empty() || path == "index.html" {
        return serve_index(&state);
    }
    match Assets::get(path) {
        Some(content) => {
            let mime = mime_guess::from_path(path).first_or_octet_stream();
            ([(header::CONTENT_TYPE, mime.as_ref())], content.data).into_response()
        }
        None => serve_index(&state),
    }
}

#[cfg(test)]
mod tests {
    use super::inject_shell_meta;

    #[test]
    fn injects_after_head_open() {
        let out = inject_shell_meta("<html><head><title>x</title></head></html>", "v2");
        assert_eq!(
            out,
            "<html><head><meta name=\"rupu-shell\" content=\"v2\"><title>x</title></head></html>"
        );
    }

    #[test]
    fn no_head_serves_unmodified() {
        assert_eq!(inject_shell_meta("<html></html>", "v2"), "<html></html>");
    }
}
```

Preserve whatever the current file's `None => …` inner-fallback status/message is if it differs; the `.fallback(crate::embed::static_handler)` line in `server.rs:93` needs **no change** (axum extracts `State<AppState>` from the router state).

- [ ] **Step 4: Run tests** — `cargo test -p rupu-cp --test embed` and `cargo test -p rupu-cp embed::` → PASS. Also `cargo clippy -p rupu-cp`.

- [ ] **Step 5: Commit** — `git commit -m "feat(cp): inject rupu-shell meta tag when serving index.html"`

---

### Task 3: `border-strong` + `surface-active` tokens

**Files:**
- Modify: `crates/rupu-cp/web/src/styles.css` (`:root` block lines 27-66, `[data-theme="dark"]` block lines 72-111)
- Modify: `crates/rupu-cp/web/tailwind.config.ts` (colors map)

**Interfaces:**
- Produces: Tailwind classes `border-border-strong`, `bg-surface-active` (and alpha variants). Tasks 6-9 and later plans consume them.

- [ ] **Step 1: Add to `:root`** (light), after `--c-border`:

```css
  /* Hairline+1: dashed affordances, "/" separators, secondary-button borders,
     hover borders. LINE/FILL ONLY — never a text colour (contrast < AA). */
  --c-border-strong: 203 213 225;
  /* Pressed/hover fill for the neutral secondary button. Fill only. */
  --c-surface-active: 203 213 225;
```

- [ ] **Step 2: Add to `[data-theme="dark"]`**, after `--c-border`:

```css
  --c-border-strong: 63 63 70;
  --c-surface-active: 46 46 51;
```

- [ ] **Step 3: Register in `tailwind.config.ts`** after the existing `border` entry:

```ts
        'border-strong': 'rgb(var(--c-border-strong) / <alpha-value>)',
        'surface-active': 'rgb(var(--c-surface-active) / <alpha-value>)',
```

- [ ] **Step 4: Verify build** — `cd crates/rupu-cp/web && npm run build` → clean.

- [ ] **Step 5: Commit** — `git commit -m "feat(cp-web): add border-strong and surface-active tokens (both themes)"`

---

### Task 4: `lib/shell.ts` flag reader

**Files:**
- Create: `crates/rupu-cp/web/src/lib/shell.ts`
- Test: `crates/rupu-cp/web/src/lib/shell.test.ts`

**Interfaces:**
- Produces: `export type ShellVersion = 'v1' | 'v2'` and `export function getShell(): ShellVersion`. Tasks 8-9 consume both.

- [ ] **Step 1: Write the failing test**:

```ts
import { afterEach, describe, expect, it } from 'vitest';
import { getShell } from './shell';

function setMeta(content: string | null) {
  document.querySelector('meta[name="rupu-shell"]')?.remove();
  if (content !== null) {
    const m = document.createElement('meta');
    m.name = 'rupu-shell';
    m.content = content;
    document.head.appendChild(m);
  }
}

describe('getShell', () => {
  afterEach(() => {
    setMeta(null);
    localStorage.removeItem('rupu.cp.shell');
  });

  it('defaults to v1 with no meta tag', () => {
    expect(getShell()).toBe('v1');
  });

  it('reads v2 from the meta tag', () => {
    setMeta('v2');
    expect(getShell()).toBe('v2');
  });

  it('treats unknown meta values as v1', () => {
    setMeta('v3');
    expect(getShell()).toBe('v1');
  });

  it('honours the DEV localStorage override when no meta tag exists', () => {
    localStorage.setItem('rupu.cp.shell', 'v2');
    expect(getShell()).toBe('v2'); // vitest runs with DEV=true
  });

  it('meta tag beats the localStorage override', () => {
    setMeta('v1');
    localStorage.setItem('rupu.cp.shell', 'v2');
    expect(getShell()).toBe('v1');
  });
});
```

- [ ] **Step 2: Run to verify failure** — `npx vitest run src/lib/shell.test.ts` → FAIL (module missing).

- [ ] **Step 3: Implement**:

```ts
// Shell v2 feature flag. Source of truth is `[ui.cp] shell` in
// ~/.rupu/config.toml; rupu-cp's embed.rs injects it as
// `<meta name="rupu-shell" content="v1"|"v2">` when serving index.html.
// The Vite dev server serves the raw index.html (no meta), so dev builds
// may opt in via `localStorage['rupu.cp.shell'] = 'v2'` — DEV only.
export type ShellVersion = 'v1' | 'v2';

export function getShell(): ShellVersion {
  const fromMeta = document
    .querySelector('meta[name="rupu-shell"]')
    ?.getAttribute('content');
  if (fromMeta === 'v1' || fromMeta === 'v2') return fromMeta;
  if (import.meta.env.DEV) {
    try {
      if (localStorage.getItem('rupu.cp.shell') === 'v2') return 'v2';
    } catch {
      /* storage unavailable (private mode) — fall through */
    }
  }
  return 'v1';
}
```

- [ ] **Step 4: Run tests** — PASS.
- [ ] **Step 5: Commit** — `git commit -m "feat(cp-web): shell flag reader (meta tag + DEV override)"`

---

### Task 5: `sidebarNavV2`

**Files:**
- Modify: `crates/rupu-cp/web/src/lib/sidebarNav.ts` (append; v1 export untouched)
- Test: `crates/rupu-cp/web/src/lib/sidebarNav.test.ts` (append a v2 describe block; do not edit v1 assertions)

**Interfaces:**
- Produces: `export type NavLeafV2`, `export const sidebarNavV2: NavLeafV2[]` (7 leaves), `export const settingsLeafV2: NavLeafV2`. Task 8's Shell consumes them.

- [ ] **Step 1: Write the failing test**:

```ts
import { sidebarNavV2, settingsLeafV2 } from './sidebarNav';

describe('sidebarNavV2', () => {
  it('has exactly the seven v2 destinations in order', () => {
    expect(sidebarNavV2.map((l) => l.to)).toEqual([
      '/overview', '/activity', '/projects', '/security', '/library', '/fleet', '/usage',
    ]);
  });
  it('pins settings separately', () => {
    expect(settingsLeafV2.to).toBe('/settings');
  });
  it('declares badge sources only where the spec assigns one', () => {
    const badges = Object.fromEntries(sidebarNavV2.map((l) => [l.to, l.badge]));
    expect(badges['/overview']).toBe('attention');
    expect(badges['/activity']).toBe('running');
    expect(badges['/projects']).toBe('projects');
    expect(badges['/security']).toBe('critical');
    expect(badges['/fleet']).toBe('unhealthy');
    expect(badges['/library']).toBeUndefined();
    expect(badges['/usage']).toBeUndefined();
  });
});
```

- [ ] **Step 2: Run to verify failure**, then **Step 3: Implement** (append to `sidebarNav.ts`; add `Activity`, `BookMarked`, `ShieldCheck` to the lucide import as needed):

```ts
/* ── Shell v2 (flat rail, 7 leaves + pinned Settings) ─────────────────── */

export type NavLeafV2 = {
  to: string;
  label: string;
  icon: LucideIcon;
  /** Which live count fills the right-aligned rail badge. Rendering is
   *  wired in Plan 3 (`/api/attention`); a leaf with no source shows no
   *  badge, and an unknown count renders nothing — never `0`. */
  badge?: 'attention' | 'running' | 'projects' | 'critical' | 'unhealthy';
};

export const sidebarNavV2: NavLeafV2[] = [
  { to: '/overview', label: 'Overview', icon: LayoutDashboard, badge: 'attention' },
  { to: '/activity', label: 'Activity', icon: Activity, badge: 'running' },
  { to: '/projects', label: 'Projects', icon: FolderGit2, badge: 'projects' },
  { to: '/security', label: 'Security', icon: ShieldCheck, badge: 'critical' },
  { to: '/library', label: 'Library', icon: BookMarked },
  { to: '/fleet', label: 'Fleet', icon: Server, badge: 'unhealthy' },
  { to: '/usage', label: 'Usage', icon: DollarSign },
];

export const settingsLeafV2: NavLeafV2 = { to: '/settings', label: 'Settings', icon: Settings };
```

- [ ] **Step 4: Run** `npx vitest run src/lib/sidebarNav.test.ts` — the v1 assertions in that file must still pass unchanged.
- [ ] **Step 5: Commit** — `git commit -m "feat(cp-web): v2 sidebar nav data (7 flat leaves + pinned settings)"`

---

### Task 6: `Brand` rail variant

**Files:**
- Modify: `crates/rupu-cp/web/src/components/Brand.tsx`
- Test: `crates/rupu-cp/web/src/components/Brand.test.tsx` (new)

**Interfaces:**
- Produces: `<Brand variant="rail" />` — 24px gradient tile + 13px wordmark, no sublabel. Default usage (`<Brand />`, `<Brand sublabel=… />`) is pixel-identical to today.

- [ ] **Step 1: Failing test**:

```tsx
import { render, screen } from '@testing-library/react';
import Brand from './Brand';

it('default variant renders wordmark and sublabel', () => {
  render(<Brand />);
  expect(screen.getByText('rupu')).toBeInTheDocument();
  expect(screen.getByText('Control Plane')).toBeInTheDocument();
});

it('rail variant renders the compact lockup without a sublabel', () => {
  render(<Brand variant="rail" />);
  expect(screen.getByText('rupu')).toBeInTheDocument();
  expect(screen.queryByText('Control Plane')).not.toBeInTheDocument();
});
```

- [ ] **Step 2: Run to verify failure** (no `variant` prop), then **Step 3: Implement**:

```tsx
interface BrandProps {
  /** Small label under the wordmark (e.g. "Control Plane"). Omit for just the mark + name. */
  sublabel?: string | null;
  /** `default` = the v1 sidebar lockup (unchanged). `rail` = the Shell v2
   *  48px rail header: 24px gradient tile + 13px wordmark, no sublabel —
   *  the caller places the trailing `cp` tag itself. */
  variant?: 'default' | 'rail';
}

export default function Brand({ sublabel = 'Control Plane', variant = 'default' }: BrandProps) {
  if (variant === 'rail') {
    return (
      <span className="flex items-center gap-2">
        <span
          aria-hidden="true"
          className="flex h-6 w-6 items-center justify-center rounded-md bg-gradient-to-br from-brand-500 to-brand-700 text-white shadow-[0_0_0_1px_rgb(var(--c-brand-500)/0.35)] font-mono text-[14px] font-light leading-none"
        >
          &#8734;
        </span>
        <span className="text-[13px] font-semibold text-ink">rupu</span>
      </span>
    );
  }
  /* existing default markup unchanged below */
```

(The gradient uses token classes; the ring shadow uses the CSS variable, not a hex. In dark theme `brand-700` resolves lighter than `brand-500`, so the mark gradients up — that is intended, per the spec.)

- [ ] **Step 4: Run tests**; **Step 5: Commit** — `git commit -m "feat(cp-web): Brand rail variant for the v2 shell header"`

---

### Task 7: shell state (scope + range)

**Files:**
- Create: `crates/rupu-cp/web/src/components/v2/shellState.tsx`
- Test: `crates/rupu-cp/web/src/components/v2/shellState.test.tsx`

**Interfaces:**
- Produces:

```ts
export type ShellRange = '7d' | '30d' | 'all';
export interface ShellStateValue {
  scope: string | null;              // workspace id; null = all projects
  range: ShellRange;                 // default '30d'
  setScope(next: string | null): void;
  setRange(next: ShellRange): void;
}
export const SHELL_STATE_KEY = 'rupu.cp.shell.v2';   // localStorage, JSON {scope, range}
export function ShellStateProvider({ children }: { children: ReactNode }): JSX.Element;
export function useShellState(): ShellStateValue;    // throws outside provider (same as useTheme)
```

Later plans read `scope`/`range` from every v2 page; Task 8's top bar renders the controls.

- [ ] **Step 1: Failing tests** — defaults (`scope: null`, `range: '30d'`), persistence roundtrip (set → new provider instance reads back), malformed stored JSON falls back to defaults, and `useShellState` outside the provider throws:

```tsx
import { render, renderHook, act } from '@testing-library/react';
import { ShellStateProvider, useShellState, SHELL_STATE_KEY } from './shellState';

const wrapper = ({ children }: { children: React.ReactNode }) => (
  <ShellStateProvider>{children}</ShellStateProvider>
);

afterEach(() => localStorage.removeItem(SHELL_STATE_KEY));

it('defaults to all-projects / 30d', () => {
  const { result } = renderHook(() => useShellState(), { wrapper });
  expect(result.current.scope).toBeNull();
  expect(result.current.range).toBe('30d');
});

it('persists and restores scope + range', () => {
  const first = renderHook(() => useShellState(), { wrapper });
  act(() => {
    first.result.current.setScope('ws-1');
    first.result.current.setRange('7d');
  });
  first.unmount();
  const second = renderHook(() => useShellState(), { wrapper });
  expect(second.result.current.scope).toBe('ws-1');
  expect(second.result.current.range).toBe('7d');
});

it('survives malformed stored JSON', () => {
  localStorage.setItem(SHELL_STATE_KEY, '{nope');
  const { result } = renderHook(() => useShellState(), { wrapper });
  expect(result.current.range).toBe('30d');
});

it('throws outside the provider', () => {
  expect(() => renderHook(() => useShellState())).toThrow();
});
```

- [ ] **Step 2: Run to verify failure**, then **Step 3: Implement** — context + provider; read localStorage once on mount (lazy `useState` initializer), write on every change, swallow storage errors (same discipline as `SidebarGroup.readGroupState`/`writeGroupState`); validate `range` against the three-value union and `scope` as `string | null` when parsing.

- [ ] **Step 4: Run tests**; **Step 5: Commit** — `git commit -m "feat(cp-web): v2 shell scope/range state with localStorage persistence"`

---

### Task 8: `ThemeToggle` icon variant + `CommandPalette` external open + v2 nav pages

**Files:**
- Modify: `crates/rupu-cp/web/src/components/theme/ThemeToggle.tsx`
- Modify: `crates/rupu-cp/web/src/components/CommandPalette.tsx`
- Test: `crates/rupu-cp/web/src/components/CommandPalette.v2.test.tsx` (new; leave the existing palette tests untouched)

**Interfaces:**
- Produces:
  - `<ThemeToggle variant="icon" />` — a 26px square icon button (existing default `variant="row"` unchanged; Layout keeps working with no prop).
  - `export const OPEN_COMMAND_PALETTE_EVENT = 'rupu:command-palette:open'` and `export function openCommandPalette(): void` in `CommandPalette.tsx` — dispatches a window `CustomEvent`; the mounted palette listens and opens. Task 9's search field calls `openCommandPalette()`.
  - `<CommandPalette shell="v2" />` prop (default `'v1'`): swaps `NAV_PAGES` for `NAV_PAGES_V2` and rewrites moved entity list targets.

- [ ] **Step 1: Failing tests** (`CommandPalette.v2.test.tsx`) — mock `../lib/api` the same way the existing palette test does; assert:
  1. `openCommandPalette()` opens the dialog (input appears).
  2. With `shell="v2"`, typing `overview` surfaces an "Overview" page item; `dashboard` does not surface a "Dashboard" page item.
  3. With `shell="v2"`, a finding entity result navigates to `/security` (spy `useNavigate` per the existing test's pattern).

- [ ] **Step 2: Run to verify failure**, then **Step 3: Implement**:

`ThemeToggle`: add `variant?: 'row' | 'icon'` prop. `icon` renders `ui/Button` `variant="ghost" size="sm"` with `className="h-[26px] w-[26px] p-0 justify-center text-ink-dim hover:text-ink"`, icon only (same `ORDER` cycling and `aria-label`).

`CommandPalette` additions:

```ts
export const OPEN_COMMAND_PALETTE_EVENT = 'rupu:command-palette:open';
export function openCommandPalette() {
  window.dispatchEvent(new CustomEvent(OPEN_COMMAND_PALETTE_EVENT));
}

const NAV_PAGES_V2: PaletteItem[] = [
  { kind: 'page', id: 'overview', title: 'Overview',     to: '/overview' },
  { kind: 'page', id: 'activity', title: 'Activity',     to: '/activity' },
  { kind: 'page', id: 'projects', title: 'Projects',     to: '/projects' },
  { kind: 'page', id: 'security', title: 'Security',     to: '/security' },
  { kind: 'page', id: 'library',  title: 'Library',      to: '/library' },
  { kind: 'page', id: 'fleet',    title: 'Fleet',        to: '/fleet' },
  { kind: 'page', id: 'usage',    title: 'Usage',        to: '/usage' },
  { kind: 'page', id: 'events',   title: 'Live Events',  to: '/events' },
  { kind: 'page', id: 'settings', title: 'Settings',     to: '/settings' },
];

/** Entity list pages that moved in v2 — detail routes are unchanged. */
const V2_LIST_REWRITE: Record<string, string> = {
  '/findings': '/security',
  '/workers': '/fleet?tab=workers',
};
```

Component: `function CommandPalette({ shell = 'v1' }: { shell?: ShellVersion })`; a `useEffect` adds/removes the `OPEN_COMMAND_PALETTE_EVENT` listener (`setOpen(true)`); pick `shell === 'v2' ? NAV_PAGES_V2 : NAV_PAGES` where `rankPalette` is called; in `go(to)`, apply `shell === 'v2' ? (V2_LIST_REWRITE[to] ?? to) : to`. Add `PAGE_ICON` entries for the new ids (`overview: LayoutDashboard, activity: Activity, security: ShieldCheck, library: BookMarked, fleet: Server, usage: DollarSign`).

- [ ] **Step 4: Run** the new test file AND the existing `CommandPalette` tests → all PASS.
- [ ] **Step 5: Commit** — `git commit -m "feat(cp-web): palette v2 nav + external open event; ThemeToggle icon variant"`

---

### Task 9: v2 Shell chrome

**Files:**
- Create: `crates/rupu-cp/web/src/components/v2/Shell.tsx`
- Test: `crates/rupu-cp/web/src/components/v2/Shell.test.tsx`

**Interfaces:**
- Consumes: `sidebarNavV2`/`settingsLeafV2` (Task 5), `Brand variant="rail"` (Task 6), `ShellStateProvider`/`useShellState` (Task 7), `openCommandPalette` + `<CommandPalette shell="v2">` + `<ThemeToggle variant="icon">` (Task 8), `api.getRegisteredHosts()`, `api.getHosts()`, `api.getProjects()`, `api.subscribeEvents`, `ConnectionState` from `components/RunEventFeed`.
- Produces: `export default function Shell()` — the v2 layout route element (rail + top bar + `<Outlet/>` + palette). Task 10 mounts it in `App.tsx`.

Layout skeleton (all classes token-based):

```tsx
// components/v2/Shell.tsx — Shell v2 chrome (docs/redesign/README.md §Shell).
// Rail 204px + 48px top bar; scope/range are shell-level state read by every
// page; the live pill reflects the real SSE connection state and degrades
// honestly (connecting/reconnecting take the warn tone via .sr-live[data-state]).
export default function Shell() {
  return (
    <ShellStateProvider>
      <div className="flex h-screen overflow-hidden">
        <Rail />
        <div className="flex min-w-0 flex-1 flex-col">
          <TopBar />
          <main className="flex-1 overflow-auto">
            <Outlet />
          </main>
        </div>
        <CommandPalette shell="v2" />
      </div>
    </ShellStateProvider>
  );
}
```

**Rail** (`<aside className="flex w-[204px] shrink-0 flex-col border-r border-border bg-panel">`):
- Header, `h-12 px-3 border-b border-border flex items-center gap-2`: `<Brand variant="rail" />` + `<span className="ml-auto font-mono text-[9px] uppercase tracking-[0.1em] text-ink-mute">cp</span>`.
- Nav `flex-1 flex flex-col gap-0.5 px-2 py-2`: one `NavLink` per `sidebarNavV2` leaf —
  `h-[30px] rounded-[5px] px-2 flex items-center gap-2 text-[13px]`; idle `text-ink-dim hover:bg-surface-hover`; active `bg-surface text-ink shadow-[inset_2px_0_0_rgb(var(--c-brand-500))]`. Icon `size={15}`. Active detection: `pathname === to || pathname.startsWith(to + '/')`. **No badges this plan** — the counts don't exist until Plan 3's `/api/attention`, and an unknown count renders nothing (spec: never `0`).
- Settings leaf pinned with `mt-auto` (same row styling), then the footer strip:
  `border-t border-border px-3 py-[9px] flex items-center gap-2 font-mono text-[10px] text-ink-mute` — a 6px status dot (`bg-status-done` when all reachable hosts are `online`, `bg-status-failed` otherwise) + `"{n} hosts"` and `" · {down} down"` only when `down > 0`; while unloaded render `— hosts`. Data: `api.getHosts()` on mount + 60s interval (`status !== 'online'` counts as down).

**TopBar** (`<header className="flex h-12 shrink-0 items-center gap-3 border-b border-border bg-panel px-4">`):
- `scope` label: `font-mono text-[11px] text-ink-mute`.
- Project scope: a native `<select>` (26px tall, `bg-surface`, `rounded-[5px]`, `border border-border`, `font-mono text-[11px] text-ink`) whose first option is `all projects` (value `''` → `setScope(null)`); options from `api.getProjects()` (`{ws_id, name}`); value bound to `useShellState().scope ?? ''`. (Custom popover styling comes with the visual-polish pass; native select is fully functional and accessible now.)
- `/` separator: `text-border-strong select-none` — wait, border-strong is fill-only: render as `<span aria-hidden className="font-mono text-[11px] text-ink-mute">/</span>`.
- Range: `ui/Segmented` `size="sm"` with options `7d/30d/all` bound to `range`/`setRange`.
- Search field: `<button onClick={openCommandPalette} className="flex h-7 max-w-[420px] flex-1 items-center gap-2 rounded-[5px] border border-border bg-bg px-2.5 text-left">` containing `Search size={13} className="text-ink-mute"`, `<span className="flex-1 truncate text-ui text-ink-mute">Jump to run, project, agent, finding…</span>`, and a `⌘K` keycap `font-mono text-[9px] rounded border border-border px-1 text-ink-mute`.
- Right group: the live pill + `<ThemeToggle variant="icon" />`.

**Live pill** — reuse the orphaned `.sr-live` CSS verbatim:

```tsx
function LivePill() {
  const [state, setState] = useState<ConnectionState>('connecting');
  useEffect(() => {
    setState('connecting');
    const unsub = api.subscribeEvents(
      () => setState('live'),
      undefined,
      () => setState('reconnecting'),
    );
    return unsub;
  }, []);
  return (
    <span className="sr-live" data-state={state}>
      <span className="sr-beacon" aria-hidden />
      {state === 'live' ? 'live' : state}
    </span>
  );
}
```

(`.sr-live[data-state="connecting"|"reconnecting"]` already takes the warn tone; `.sr-beacon` is already reduced-motion-guarded at `styles.css:397`.)

- [ ] **Step 1: Failing tests** (`Shell.test.tsx`) — `vi.mock('../../lib/api')` with `getHosts: async () => []`, `getProjects: async () => []`, `getRegisteredHosts: async () => []`, `subscribeEvents: () => () => {}`, plus the palette's fetch-on-open functions returning `[]`. Render inside `MemoryRouter` with a stub `<Route element={<Shell/>}><Route path="*" element={<div>body</div>}/></Route>`. Assert:
  1. All seven nav labels + Settings render, in order.
  2. Rail shows `rupu` + `cp`.
  3. The row matching the current route carries `aria-current="page"` (NavLink default).
  4. The search button opens the palette (palette input appears after click).
  5. Live pill initially renders `connecting` with `data-state="connecting"`; after invoking the mocked subscribe's `onEvent`, renders `live`.
  6. Range segmented reflects and updates `useShellState` (`30d` default; click `7d` persists to `localStorage['rupu.cp.shell.v2']`).
- [ ] **Step 2: Run to verify failure**, then **Step 3: Implement** per the skeleton above.
- [ ] **Step 4: Run tests** → PASS. Also re-run `src/lib/shell.test.ts` and Task 7/8 suites.
- [ ] **Step 5: Commit** — `git commit -m "feat(cp-web): Shell v2 chrome — rail, top bar, live pill, scope/range"`

---

### Task 10: routes, redirects, composite pages, `App.tsx` branch

**Files:**
- Create: `crates/rupu-cp/web/src/components/v2/pages/Composite.tsx`
- Create: `crates/rupu-cp/web/src/components/v2/pages/ActivityV2.tsx`, `SecurityV2.tsx`, `LibraryV2.tsx`, `FleetV2.tsx`
- Modify: `crates/rupu-cp/web/src/App.tsx`
- Test: `crates/rupu-cp/web/src/components/v2/pages/Composite.test.tsx`, `crates/rupu-cp/web/src/AppRoutes.v2.test.tsx`

**Interfaces:**
- Consumes: `getShell()` (Task 4), `Shell` (Task 9), every existing lazy page import in `App.tsx`.
- Produces: `export function AppRoutes({ shell }: { shell: ShellVersion })` from `App.tsx` (used by tests; `App` wraps it in `BrowserRouter` + `ErrorBoundary` and passes `getShell()`).

**`Composite.tsx`** — the generic tabbed wrapper (interim IA carrier; later plans replace these bodies with the real v2 pages):

```tsx
// Interim v2 destination page: a Segmented tab bar over existing page
// bodies, so the 7-leaf IA is complete and capability-equal from Plan 1.
// The active tab lives in the `?tab=` search param so redirects from old
// routes can deep-link a specific tab and the URL stays shareable.
import { Suspense, type ReactNode } from 'react';
import { useSearchParams } from 'react-router-dom';
import { Segmented } from '../../ui/Segmented';
import { Spinner } from '../../ui/Spinner';

export interface CompositeTab {
  value: string;
  label: string;
  element: ReactNode;
}

export function Composite({ title, tabs, defaultTab }: {
  title: string;
  tabs: CompositeTab[];
  defaultTab: string;
}) {
  const [params, setParams] = useSearchParams();
  const raw = params.get('tab');
  const tab = tabs.some((t) => t.value === raw) ? (raw as string) : defaultTab;
  const active = tabs.find((t) => t.value === tab) ?? tabs[0];
  return (
    <div className="flex h-full flex-col">
      <div className="flex items-center gap-3 border-b border-border bg-panel px-4 py-2">
        <h1 className="text-[16px] font-semibold tracking-[-0.01em] text-ink">{title}</h1>
        <Segmented
          size="sm"
          ariaLabel={`${title} sections`}
          options={tabs.map((t) => ({ value: t.value, label: t.label }))}
          value={tab}
          onChange={(next) =>
            setParams((p) => {
              const q = new URLSearchParams(p);
              q.set('tab', next);
              return q;
            }, { replace: true })
          }
        />
      </div>
      <div className="min-h-0 flex-1 overflow-auto">
        <Suspense fallback={<div className="flex h-48 items-center justify-center"><Spinner size="md" label="Loading…" /></div>}>
          {active.element}
        </Suspense>
      </div>
    </div>
  );
}
```

Each destination page imports the existing lazy pages (e.g. `ActivityV2.tsx`):

```tsx
import { lazy } from 'react';
import { Composite } from './Composite';

const AgentRuns = lazy(() => import('../../../pages/runs/AgentRuns'));
const WorkflowRuns = lazy(() => import('../../../pages/runs/WorkflowRuns'));
const AutoflowRuns = lazy(() => import('../../../pages/runs/AutoflowRuns'));
const Sessions = lazy(() => import('../../../pages/Sessions'));

export default function ActivityV2() {
  return (
    <Composite
      title="Activity"
      defaultTab="workflows"
      tabs={[
        { value: 'agents', label: 'agents', element: <AgentRuns /> },
        { value: 'workflows', label: 'workflows', element: <WorkflowRuns /> },
        { value: 'autoflows', label: 'autoflows', element: <AutoflowRuns /> },
        { value: 'sessions', label: 'sessions', element: <Sessions /> },
      ]}
    />
  );
}
```

Same shape for:
- `SecurityV2` — title `Security`, default `findings`; tabs findings→`pages/Findings`, coverage→`pages/Coverage`, catalog→`pages/CoverageTemplates`.
- `LibraryV2` — title `Library`, default `agents`; tabs agents→`pages/Agents`, workflows→`pages/Workflows`, autoflows→`pages/AutoflowsDefs`. (contracts tab arrives in Plan 5.)
- `FleetV2` — title `Fleet`, default `hosts`; tabs hosts→`pages/Hosts`, workers→`pages/Workers`.

**`App.tsx`** — extract the routes into an exported component and branch the layout element:

```tsx
const ShellV2 = React.lazy(() => import('./components/v2/Shell'));
const ActivityV2 = React.lazy(() => import('./components/v2/pages/ActivityV2'));
const SecurityV2 = React.lazy(() => import('./components/v2/pages/SecurityV2'));
const LibraryV2 = React.lazy(() => import('./components/v2/pages/LibraryV2'));
const FleetV2 = React.lazy(() => import('./components/v2/pages/FleetV2'));

export function AppRoutes({ shell }: { shell: ShellVersion }) {
  const v2 = shell === 'v2';
  const layoutEl = v2
    ? <Suspense fallback={<PageFallback />}><ShellV2 /></Suspense>
    : <Layout />;
  return (
    <Routes>
      <Route element={layoutEl}>
        <Route index element={<Navigate to={v2 ? '/overview' : '/dashboard'} replace />} />

        {/* v2 destinations — registered under BOTH shells so deep links work */}
        <Route path="/overview" element={page(<Dashboard />)} />
        <Route path="/activity" element={page(<ActivityV2 />)} />
        <Route path="/security" element={page(<SecurityV2 />)} />
        <Route path="/library" element={page(<LibraryV2 />)} />
        <Route path="/fleet" element={page(<FleetV2 />)} />

        {/* v1 list paths — redirect into the v2 IA only when the flag is on */}
        <Route path="/dashboard" element={v2 ? <Navigate to="/overview" replace /> : page(<Dashboard />)} />
        <Route path="/runs/agents" element={v2 ? <Navigate to="/activity?tab=agents" replace /> : page(<AgentRuns />)} />
        <Route path="/runs/workflows" element={v2 ? <Navigate to="/activity?tab=workflows" replace /> : page(<WorkflowRuns />)} />
        <Route path="/runs/autoflows" element={v2 ? <Navigate to="/activity?tab=autoflows" replace /> : page(<AutoflowRuns />)} />
        <Route path="/runs" element={<Navigate to={v2 ? '/activity' : '/runs/workflows'} replace />} />
        <Route path="/sessions" element={v2 ? <Navigate to="/activity?tab=sessions" replace /> : page(<Sessions />)} />
        <Route path="/findings" element={v2 ? <Navigate to="/security?tab=findings" replace /> : page(<Findings />)} />
        <Route path="/coverage" element={v2 ? <Navigate to="/security?tab=coverage" replace /> : page(<Coverage />)} />
        <Route path="/coverage/templates" element={v2 ? <Navigate to="/security?tab=catalog" replace /> : page(<CoverageTemplates />)} />
        <Route path="/agents" element={v2 ? <Navigate to="/library?tab=agents" replace /> : page(<Agents />)} />
        <Route path="/workflows" element={v2 ? <Navigate to="/library?tab=workflows" replace /> : page(<Workflows />)} />
        <Route path="/autoflows" element={v2 ? <Navigate to="/library?tab=autoflows" replace /> : page(<AutoflowsDefs />)} />
        <Route path="/hosts" element={v2 ? <Navigate to="/fleet" replace /> : page(<Hosts />)} />
        <Route path="/workers" element={v2 ? <Navigate to="/fleet?tab=workers" replace /> : page(<Workers />)} />

        {/* everything below is UNCHANGED from today — detail routes, events,
            usage, settings, projects, transcript. Keep the existing order:
            /runs/agents|workflows|autoflows above /runs/:id; /agents/new
            above /agents/:name. */}
        …existing routes verbatim…
      </Route>
    </Routes>
  );
}

export default function App() {
  const shell = getShell();
  return (
    <BrowserRouter>
      <ErrorBoundary>
        <AppRoutes shell={shell} />
      </ErrorBoundary>
    </BrowserRouter>
  );
}
```

where `page(el)` is a tiny helper replacing the repeated `<Suspense fallback={<PageFallback/>}>{el}</Suspense>` wrapper (pure refactor of the existing pattern — do not change lazy-import granularity).

**Route-order caution:** `/runs/agents` etc. must stay registered before `/runs/:id`, and `/agents/new` before `/agents/:name` — the redirects replace the elements in place, not the order. `/coverage/:target/*` and `/sessions/:id` remain and are NOT redirected.

- [ ] **Step 1: Failing tests**:

`Composite.test.tsx` — renders tabs, defaults correctly, unknown `?tab=` falls back to default, clicking a segment updates `?tab=` (use `MemoryRouter initialEntries={['/x?tab=b']}` and stub elements `<div>A</div>`/`<div>B</div>`).

`AppRoutes.v2.test.tsx` — mock `./lib/api` broadly (every function the shells/pages touch returning empty resolves; `subscribeEvents: () => () => {}`). Render with a location spy:

```tsx
function LocationSpy() {
  const loc = useLocation();
  return <div data-testid="loc">{loc.pathname + loc.search}</div>;
}
```

Cases:
1. `shell="v2"`, start `/dashboard` → spy shows `/overview`.
2. `shell="v2"`, start `/runs/agents` → `/activity?tab=agents`.
3. `shell="v2"`, start `/findings` → `/security?tab=findings`.
4. `shell="v2"`, start `/workers` → `/fleet?tab=workers`.
5. `shell="v1"`, start `/dashboard` → stays `/dashboard`.
6. `shell="v1"`, start `/overview` → stays `/overview` (v2 routes exist under v1).
7. `shell="v2"`, start `/runs/abc123` → stays `/runs/abc123` (detail untouched).
8. `shell="v2"`, start `/events` → stays `/events` (wall display survives).

- [ ] **Step 2: Run to verify failure**, then **Step 3: Implement** as specified.
- [ ] **Step 4: Run** the new tests, then the **entire web suite** (`npx vitest run`) — v1 page tests must be untouched and green.
- [ ] **Step 5: Commit** — `git commit -m "feat(cp-web): v2 routes, redirects, composite pages, App shell branch"`

---

### Task 11: full verification + PR

- [ ] **Step 1: Rust** — `cargo test -p rupu-config && cargo test -p rupu-cp && cargo clippy --workspace --all-targets` → clean. (Measure the actual baseline first if anything looks pre-broken — never assume red.)
- [ ] **Step 2: Web** — `cd crates/rupu-cp/web && npx vitest run && npm run build` → clean.
- [ ] **Step 3: Runtime validation** — `make cp-web` (rebuild embedded UI) then run `rupu cp serve` from a checkout; verify in a browser:
  - default config → v1 UI identical to today;
  - `rupu config set ui.cp.shell v2` + refresh (no server restart) → v2 shell: 7-leaf rail, top bar, live pill goes green, ⌘K and the search field open the palette, every destination + tab loads real data, old bookmark URLs redirect, `/runs/:id` and `/events` still work;
  - toggle dark/light — no unthemed surfaces in the new chrome.
- [ ] **Step 4: PR** — push `feat/cp-shell-v2-plan-1`, open a PR against `main` titled `CP Shell v2 — Plan 1: flag, tokens, shell chrome, route map`, body linking `docs/redesign/README.md` + the arc doc, noting: v1 untouched, v2 behind `[ui.cp] shell`, interim composite bodies to be replaced by Plans 2-5.

---

## Self-review notes

- Spec coverage this plan: rollout flag + wiring (§Rollout 1-4), token additions (§Design tokens), rail + top bar (§Shell), route map + palette (§IA), scope/range state (§Interactions). Deliberately deferred: rail badges (Plan 3 — counts need `/api/attention`; spec says unknown renders nothing, so an empty badge slot is spec-compliant), custom scope-selector popover (native select now, visual polish later), the v2 page bodies themselves (Plans 2-5), `[ui.cp].shell` in Settings UI form (arc follow-up; `rupu config set` + raw TOML editor both work today).
- Type consistency: `ShellVersion` (Task 4) is consumed by Tasks 8-10; `ShellRange`/`useShellState` (Task 7) by Task 9; `NavLeafV2.badge` union matches the arc doc's badge sources; `CompositeTab.value` strings match the redirect `?tab=` values exactly (`agents/workflows/autoflows/sessions`, `findings/coverage/catalog`, `hosts/workers`).
