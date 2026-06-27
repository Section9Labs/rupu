# CP Target Picker — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development. Steps use `- [ ]`.

**Goal:** Replace the Workspace/Directory/Repository toggle in both Run modals with a single Spotlight-style `TargetPicker` (grouped, colored, fuzzy, keyboard-nav) over Projects + Repos + live Directory browse + free-text.

**Architecture:** Pure logic (`lib/fuzzy.ts`, `lib/targetItems.ts`) is unit-tested; `TargetPicker.tsx` composes them with `api.browseDir` (live) + keyboard nav + grouped render; the two sheets swap their target block for `<TargetPicker>` and read `item.resolved.{target,working_dir}`. Frontend-only.

**Tech Stack:** React 18 + TS + Vite + Vitest + Tailwind.

Spec: `docs/superpowers/specs/2026-06-27-rupu-cp-target-picker-design.md`.

## Global Constraints
- All under `crates/rupu-cp/web`. vitest `globals: false`: pure tests node env; component tests `// @vitest-environment jsdom` + explicit imports + `afterEach(cleanup)`.
- Reuse existing api: `getProjects(): ProjectRow[]` (`.path`,`.name`), `getRepos(): RepoEntry[]` (`.platform`,`.repo`,`.default_branch`), `browseDir(path?): {path,parent,dirs:[{name,path}]}`.
- Color tokens already in the project: emerald/violet/slate `bg-*-50 text-*-700 ring-*-200`, plus `bg-brand-*`, `text-ink`, `text-ink-dim`, `text-ink-mute`, `border-border`, `bg-panel`.
- Verify each task with `npx tsc --noEmit` before commit; full `npx vitest run` + `npm run build` at the end.

---

## Task 1: `lib/fuzzy.ts` (matcher)

**Files:** Create `crates/rupu-cp/web/src/lib/fuzzy.ts` + `fuzzy.test.ts`.

**Interfaces (produces):** `fuzzyScore(query: string, text: string): { score: number; matched: number[] } | null`.

- [ ] **Step 1: Failing test** — `fuzzy.test.ts`:
```ts
import { describe, it, expect } from 'vitest';
import { fuzzyScore } from './fuzzy';

describe('fuzzyScore', () => {
  it('empty query scores 0 with no matches', () => {
    expect(fuzzyScore('', 'anything')).toEqual({ score: 0, matched: [] });
  });
  it('returns null when chars are missing', () => {
    expect(fuzzyScore('xyz', 'abc')).toBeNull();
  });
  it('exact substring outranks scattered subsequence', () => {
    const a = fuzzyScore('api', 'github:acme/api')!;   // substring
    const b = fuzzyScore('api', 'a-p-i-zzzz')!;        // subsequence
    expect(a).not.toBeNull();
    expect(b).not.toBeNull();
    expect(a.score).toBeGreaterThan(b.score);
  });
  it('records matched indices for a substring', () => {
    const r = fuzzyScore('cd', 'abcde')!;
    expect(r.matched).toEqual([2, 3]);
  });
  it('rewards word-boundary starts', () => {
    const boundary = fuzzyScore('w', 'foo/web')!;       // 'w' after '/'
    const mid = fuzzyScore('w', 'crawl')!;              // 'w' mid-word
    expect(boundary.score).toBeGreaterThan(mid.score);
  });
});
```

- [ ] **Step 2:** `cd crates/rupu-cp/web && npx vitest run src/lib/fuzzy.test.ts` → FAILS.

- [ ] **Step 3: Implement** `fuzzy.ts`:
```ts
// Small fuzzy matcher (ported from Okesu's command palette): exact-substring
// beats subsequence; subsequence rewards matches at word boundaries. Returns
// the matched char indices in `text` for highlighting, or null on no match.
const BOUNDARY = /[\s\-/_.:]/;

export function fuzzyScore(
  query: string,
  text: string,
): { score: number; matched: number[] } | null {
  if (query === '') return { score: 0, matched: [] };
  const q = query.toLowerCase();
  const t = text.toLowerCase();

  // Exact substring.
  const idx = t.indexOf(q);
  if (idx >= 0) {
    const matched = Array.from({ length: q.length }, (_, i) => idx + i);
    const score = 1000 + (idx === 0 ? 200 : 0);
    return { score, matched };
  }

  // Subsequence walk.
  let score = 0;
  let qi = 0;
  const matched: number[] = [];
  for (let ti = 0; ti < t.length && qi < q.length; ti++) {
    if (t[ti] === q[qi]) {
      score += 10;
      if (ti === 0 || BOUNDARY.test(t[ti - 1])) score += 5;
      matched.push(ti);
      qi++;
    }
  }
  if (qi < q.length) return null;
  return { score, matched };
}
```

- [ ] **Step 4:** `npx vitest run src/lib/fuzzy.test.ts` → PASS.
- [ ] **Step 5: Commit** — `git add -A && git commit -m "feat(cp/web): fuzzy matcher for the target picker"`

---

## Task 2: `lib/targetItems.ts` (model + builders)

**Files:** Create `crates/rupu-cp/web/src/lib/targetItems.ts` + `targetItems.test.ts`.

**Interfaces (produces):**
```ts
export type TargetKind = 'workspace' | 'project' | 'repo' | 'directory';
export interface TargetItem {
  kind: TargetKind;
  label: string;
  sublabel?: string;
  resolved: { target?: string; working_dir?: string };
}
export const WORKSPACE_ITEM: TargetItem;
export function projectItems(projects: ProjectRow[]): TargetItem[];
export function repoItems(repos: RepoEntry[]): TargetItem[];
export function dirItems(dirs: FsEntry[]): TargetItem[];
export function inferFreeTextItem(query: string): TargetItem | null;
export function looksLikePath(query: string): boolean;
```

- [ ] **Step 1: Failing test** — `targetItems.test.ts`:
```ts
import { describe, it, expect } from 'vitest';
import {
  WORKSPACE_ITEM, projectItems, repoItems, dirItems, inferFreeTextItem, looksLikePath,
} from './targetItems';

describe('targetItems', () => {
  it('workspace resolves to neither field', () => {
    expect(WORKSPACE_ITEM.kind).toBe('workspace');
    expect(WORKSPACE_ITEM.resolved).toEqual({});
  });
  it('project → working_dir = path', () => {
    const [it0] = projectItems([{ ws_id: 'w', name: 'rupu', path: '/Code/rupu' } as any]);
    expect(it0).toMatchObject({ kind: 'project', label: 'rupu', sublabel: '/Code/rupu', resolved: { working_dir: '/Code/rupu' } });
  });
  it('repo → target = platform:repo', () => {
    const [it0] = repoItems([{ platform: 'github', repo: 'o/r', default_branch: 'main', private: false }]);
    expect(it0).toMatchObject({ kind: 'repo', label: 'github:o/r', resolved: { target: 'github:o/r' } });
  });
  it('dir → working_dir = path', () => {
    const [it0] = dirItems([{ name: 'crates', path: '/x/crates' }]);
    expect(it0).toMatchObject({ kind: 'directory', label: 'crates', sublabel: '/x/crates', resolved: { working_dir: '/x/crates' } });
  });
  it('free text: platform:owner/repo → repo target', () => {
    expect(inferFreeTextItem('github:acme/api')).toMatchObject({ kind: 'repo', resolved: { target: 'github:acme/api' } });
  });
  it('free text: paths → directory working_dir', () => {
    for (const p of ['/abs/x', '~/y', './rel', 'a/b']) {
      expect(inferFreeTextItem(p)).toMatchObject({ kind: 'directory', resolved: { working_dir: p } });
    }
    expect(looksLikePath('/abs')).toBe(true);
    expect(looksLikePath('plainword')).toBe(false);
  });
  it('free text: empty → null', () => {
    expect(inferFreeTextItem('  ')).toBeNull();
  });
});
```

- [ ] **Step 2:** `npx vitest run src/lib/targetItems.test.ts` → FAILS.

- [ ] **Step 3: Implement** `targetItems.ts`:
```ts
import type { ProjectRow, RepoEntry, FsEntry } from './api';

export type TargetKind = 'workspace' | 'project' | 'repo' | 'directory';
export interface TargetItem {
  kind: TargetKind;
  label: string;
  sublabel?: string;
  resolved: { target?: string; working_dir?: string };
}

export const WORKSPACE_ITEM: TargetItem = {
  kind: 'workspace',
  label: 'This workspace',
  sublabel: 'run in the control-plane working directory',
  resolved: {},
};

export function projectItems(projects: ProjectRow[]): TargetItem[] {
  return projects.map((p) => ({
    kind: 'project',
    label: p.name,
    sublabel: p.path,
    resolved: { working_dir: p.path },
  }));
}

export function repoItems(repos: RepoEntry[]): TargetItem[] {
  return repos.map((r) => {
    const ref = `${r.platform}:${r.repo}`;
    return { kind: 'repo', label: ref, sublabel: r.default_branch, resolved: { target: ref } };
  });
}

export function dirItems(dirs: FsEntry[]): TargetItem[] {
  return dirs.map((d) => ({
    kind: 'directory',
    label: d.name,
    sublabel: d.path,
    resolved: { working_dir: d.path },
  }));
}

const REPO_RE = /^[a-z][a-z0-9]*:.+/;

export function looksLikePath(query: string): boolean {
  const q = query.trim();
  return q.startsWith('/') || q.startsWith('~') || q.startsWith('.') || q.includes('/');
}

/** Synthesize an item from raw text so the user can always pick what they typed. */
export function inferFreeTextItem(query: string): TargetItem | null {
  const q = query.trim();
  if (!q) return null;
  if (REPO_RE.test(q) && !q.startsWith('/') && !q.startsWith('~') && !q.startsWith('.')) {
    return { kind: 'repo', label: q, resolved: { target: q } };
  }
  if (looksLikePath(q)) {
    return { kind: 'directory', label: q, sublabel: 'directory', resolved: { working_dir: q } };
  }
  return null;
}
```
(Note: `REPO_RE` matches `github:acme/api`; the leading path guards prevent a `/a:b`-style path from being mis-read as a repo. `a/b` has no `:` so it falls to the path branch — correct.)

- [ ] **Step 4:** `npx vitest run src/lib/targetItems.test.ts` → PASS.
- [ ] **Step 5: Commit** — `git add -A && git commit -m "feat(cp/web): TargetItem model + source builders + free-text inference"`

---

## Task 3: `TargetPicker` component

**Files:** Create `crates/rupu-cp/web/src/components/TargetPicker.tsx` + `TargetPicker.test.tsx`.

**Interfaces (produces):** `<TargetPicker value onChange disabled />`; default export. Consumes `fuzzyScore`, the `targetItems` builders, `api.{getProjects,getRepos,browseDir}`. Exposes a pure `rankAndGroup(items, query): { kind: TargetKind; items: { item: TargetItem; matched: number[] }[] }[]` for testing (groups in fixed order, cap 6/group, fuzzy-filtered+sorted).

- [ ] **Step 1: Failing test** — `TargetPicker.test.tsx`:
```ts
import { describe, it, expect } from 'vitest';
import { rankAndGroup } from './TargetPicker';
import { WORKSPACE_ITEM, type TargetItem } from '../lib/targetItems';

const items: TargetItem[] = [
  WORKSPACE_ITEM,
  { kind: 'project', label: 'rupu', sublabel: '/Code/rupu', resolved: { working_dir: '/Code/rupu' } },
  { kind: 'project', label: 'okesu', sublabel: '/Code/Okesu', resolved: { working_dir: '/Code/Okesu' } },
  { kind: 'repo', label: 'github:acme/api', sublabel: 'main', resolved: { target: 'github:acme/api' } },
];

describe('rankAndGroup', () => {
  it('empty query keeps all, workspace group first', () => {
    const groups = rankAndGroup(items, '');
    expect(groups[0].kind).toBe('workspace');
    expect(groups.map((g) => g.kind)).toEqual(['workspace', 'project', 'repo']);
  });
  it('filters by fuzzy query across label/sublabel', () => {
    const groups = rankAndGroup(items, 'rupu');
    const flat = groups.flatMap((g) => g.items.map((x) => x.item.label));
    expect(flat).toContain('rupu');
    expect(flat).not.toContain('okesu');
  });
});
```

- [ ] **Step 2:** `npx vitest run src/components/TargetPicker.test.tsx` → FAILS.

- [ ] **Step 3: Implement** `TargetPicker.tsx`. Requirements:
- Export pure `rankAndGroup(items, query)`: score each item via `fuzzyScore(query, item.label)` (fallback to `fuzzyScore(query, item.sublabel)` when label misses; keep best matched indices for the label only), drop nulls, sort by score desc within kind, group in fixed order `['workspace','project','repo','directory']`, cap 6 per group. Workspace item always passes when query is empty or matches "this workspace".
- State: `query` (string), `active` (flat index), loaded `projects`/`repos` (from `getProjects`/`getRepos` on mount, tolerate failure → []), and live `dirs` from `browseDir`.
- Live dirs: when `looksLikePath(query)`, debounce ~150ms and call `api.browseDir(query)` (the backend canonicalizes; if it errors, leave dirs empty). Map via `dirItems`.
- Build the candidate list each render: `[WORKSPACE_ITEM, ...projectItems(projects), ...repoItems(repos), ...dirItems(dirs)]`, plus `inferFreeTextItem(query)` prepended into its kind group when the query doesn't exactly equal an existing item's label. Then `rankAndGroup(candidates, query)`.
- Render: a text input (value = `query`; on focus shows the list; placeholder "search projects, repos, or a path…"), and a dropdown with group headers (colored chip per kind: project=emerald, repo=violet, directory=slate, workspace=slate/neutral) and rows showing label (with matched chars bolded via the `matched` indices) + faint sublabel. Active row highlighted.
- Keyboard: ArrowDown/Up move `active` across the flat ordered rows; Enter selects (`onChange(item)`, set `query` to item.label, close); Escape closes.
- Selecting an item: `onChange(item)`; reflect the selection (e.g., set input text to the item's label; if workspace, show "This workspace").
- Colors reuse the chip idiom: `inline-flex items-center rounded px-1.5 py-0.5 text-[11px] font-medium ring-1` with `bg-emerald-50 text-emerald-700 ring-emerald-200` (project), `bg-violet-50 text-violet-700 ring-violet-200` (repo), `bg-slate-100 text-ink-mute ring-slate-200` (directory/workspace).
- Matched-char highlight: render the label splitting on `matched` indices, wrapping matched chars in `<mark className="bg-transparent text-brand-700 font-semibold">`.
Provide a `fieldCls` consistent with the sheets (border/rounded/text-[13px]).

- [ ] **Step 4:** `npx vitest run src/components/TargetPicker.test.tsx` → PASS; `npx tsc --noEmit` clean.
- [ ] **Step 5: Commit** — `git add -A && git commit -m "feat(cp/web): TargetPicker spotlight component"`

---

## Task 4: Wire into `LauncherSheet`

**Files:** `crates/rupu-cp/web/src/components/LauncherSheet.tsx`.

- [ ] **Step 1:** Replace the target block. Read the file; then:
- Remove `targetMode`/`target`/`workingDir` state, the mode-toggle JSX, the `DirectoryPicker`/`Combobox` imports + usage, and the repo-options fetch (the picker fetches its own).
- Add `import TargetPicker from './TargetPicker';` and `import { WORKSPACE_ITEM, type TargetItem } from '../lib/targetItems';`.
- Add `const [target, setTarget] = useState<TargetItem>(WORKSPACE_ITEM);`.
- Render in the Target section:
```tsx
          <div>
            <span className="mb-1 block text-[12px] font-semibold uppercase tracking-wide text-ink-dim">Target</span>
            <TargetPicker value={target} onChange={setTarget} disabled={launching} />
          </div>
```
- In `onLaunch`, replace the target/working_dir lines with:
```tsx
        target: target.resolved.target,
        working_dir: target.resolved.working_dir,
```

- [ ] **Step 2:** `npx tsc --noEmit && npx vitest run && npm run build`. If an existing LauncherSheet test referenced the old toggle ("This workspace"/"Directory"/"Repository" buttons) or the Target combobox `aria-label`, update those assertions to the new picker (e.g. assert the launch payload via a selected `TargetItem`, or that the picker input renders). Keep tests green and meaningful.
- [ ] **Step 3: Commit** — `git add -A && git commit -m "feat(cp/web): use TargetPicker in LauncherSheet"`

---

## Task 5: Wire into `AgentLauncherSheet`

**Files:** `crates/rupu-cp/web/src/components/AgentLauncherSheet.tsx`.

- [ ] **Step 1:** Same swap as Task 4. Change `buildAgentLaunch` to take the resolved target instead of `(targetMode, target, workingDir)`:
```tsx
export function buildAgentLaunch(prompt: string, mode: LaunchMode, target: TargetItem): AgentLaunch {
  const out: AgentLaunch = { mode };
  const p = prompt.trim();
  if (p) out.prompt = p;
  if (target.resolved.target) out.target = target.resolved.target;
  if (target.resolved.working_dir) out.working_dir = target.resolved.working_dir;
  return out;
}
```
Update its test (`AgentLauncherSheet.test.tsx`) to the new signature: pass `TargetItem`s (a repo item → `{target}`; a directory item → `{working_dir}`; `WORKSPACE_ITEM` → neither). Render the `<TargetPicker>` in the sheet and use `target.resolved` in `onLaunch`.

- [ ] **Step 2:** `npx tsc --noEmit && npx vitest run && npm run build` green.
- [ ] **Step 3: Commit** — `git add -A && git commit -m "feat(cp/web): use TargetPicker in AgentLauncherSheet"`

---

## Task 6: Remove dead components + verify + PR

- [ ] **Step 1:** Grep for remaining usages: `grep -rn "DirectoryPicker\|from './Combobox'\|from '../components/Combobox'" crates/rupu-cp/web/src`. If `DirectoryPicker` and/or `Combobox` are now unused, delete the component + its test file. If still used elsewhere, leave them. (`repoToOption` lived in LauncherSheet — remove it if now unused.)
- [ ] **Step 2:** `cd crates/rupu-cp/web && npx tsc --noEmit && npx vitest run && npm run build` → all green.
- [ ] **Step 3:** Manual: `make cp-web && rupu cp serve`; open a workflow → Run → Target box: empty shows This workspace + Projects + Repos; type a project name (fuzzy), a `github:owner/repo`, and a `/path` (drills dirs); pick each and Launch → run uses the right working_dir/target. Same on an agent.
- [ ] **Step 4: PR** — `gh pr create --title "feat(cp): unified Spotlight-style Run target picker" --body "…"`

## Self-review notes
- Spec coverage: fuzzy (T1), model+builders+free-text (T2), component+group/rank (T3), sheet wiring (T4/T5), cleanup+verify (T6).
- Type parity: `TargetItem.resolved.{target,working_dir}` maps to the existing `launchRun`/`launchAgent` opts unchanged — no backend/api change.
- The picker fetches its own projects/repos; sheets no longer hold repo-options state.
