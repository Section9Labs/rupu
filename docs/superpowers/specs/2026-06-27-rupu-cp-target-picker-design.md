# rupu CP web — unified Spotlight-style Run target picker

Date: 2026-06-27
Status: approved (design)

## Problem

The Run modals (`LauncherSheet` for workflows, `AgentLauncherSheet` for agents)
make the user pre-classify the run target: click a Workspace / Directory /
Repository toggle, then use a different picker per mode (`DirectoryPicker`,
repo `Combobox`). Inspired by Okesu's Cmd-K command palette, we replace this
with a **single search input** whose *results* are classified — grouped and
color-labelled by category — with fuzzy matching, matched-char highlighting,
and keyboard navigation. The user types once; the picker resolves the choice
to the correct backend field.

Reference: Okesu `web/src/components/CommandPalette.tsx` — single input,
hand-rolled fuzzy matcher (exact-substring + subsequence + word-boundary
bonus), results grouped by category and capped per group, category colors,
matched-char highlight, ↑/↓/Enter/Esc.

## Decisions (from brainstorming)

- **Unified Spotlight input** replacing the mode toggle entirely (grouped,
  colored, fuzzy, keyboard-nav).
- **Sources (v1):** Projects (`getProjects`), Repositories (`getRepos`),
  Directory browse (`browseDir`), plus free-text. No Issues/PRs (would need a
  new CP API — deferred). Frontend-only; no backend changes.

## Component: `TargetPicker` (`crates/rupu-cp/web/src/components/TargetPicker.tsx`)

Single search box → grouped, colored result list. Used by both Run sheets.

### Model
```ts
export type TargetKind = 'workspace' | 'project' | 'repo' | 'directory';
export interface TargetItem {
  kind: TargetKind;
  /** Primary text (project name, "platform:owner/repo", dir name, or "This workspace"). */
  label: string;
  /** Faint secondary (abs path, default branch, …). */
  sublabel?: string;
  /** What the parent sends to the launch endpoint. */
  resolved: { target?: string; working_dir?: string };
}
```
- Workspace → `resolved: {}` (neither field; runs in cp-serve cwd).
- Project / Directory → `resolved: { working_dir: <abs path> }`.
- Repo → `resolved: { target: "platform:owner/repo" }`.

The picker hides the `target` vs `working_dir` split; the parent reads
`item.resolved` on launch.

### Props
```ts
{ value: TargetItem; onChange: (item: TargetItem) => void; disabled?: boolean }
```
Default `value` = the static Workspace item (`{ kind:'workspace', label:'This workspace', resolved:{} }`).

### Sources & build (`buildTargetItems`)
Preload on mount: `getProjects()` and `getRepos()` (tolerate failures → empty,
like today). `browseDir()` is queried live for path-like queries (see below).
A pure builder maps the raw lists to `TargetItem[]`:
- Workspace: one static item, always present (top).
- Projects: `getProjects()` → `{ kind:'project', label:p.name, sublabel:p.path, resolved:{working_dir:p.path} }`.
- Repositories: `getRepos()` → `{ kind:'repo', label:`${r.platform}:${r.repo}`, sublabel:r.default_branch, resolved:{target:`${r.platform}:${r.repo}`} }`.
- Directory: `browseDir(dir).dirs` → `{ kind:'directory', label:d.name, sublabel:d.path, resolved:{working_dir:d.path} }`.

### Free-text (always selectable)
When the query is non-empty and doesn't exactly match a known item, synthesize
a leading item from the raw text, inferring kind:
- matches `^[a-z][a-z0-9]*:.+` (e.g. `github:owner/repo`) → Repo item
  (`resolved.target = query`).
- looks like a path — starts with `/`, `~`, or `.`, or contains `/` — →
  Directory item (`resolved.working_dir = query`).
- empty → Workspace.
So a launch target is always an explicit pick (the user can always select their
typed value), never an ambiguous launch-time guess.

### Live directory drill
When the query is path-like, debounce-call `browseDir(dirnameOf(query))` and
feed matching subdirectories into the Directory group, so typing/selecting
drills the tree. Non-path queries leave Directory empty (or a one-line hint).

### Filtering / ranking (`lib/fuzzy.ts`)
A small ported matcher `fuzzyScore(query, text): { score, matched: number[] } | null`:
exact-substring (big score, +bonus at index 0) → subsequence walk (+per-char,
+word-boundary bonus on `\s-/_.:`) → null on miss. Items are scored on
`label` (fallback `sublabel`), filtered to matches (empty query = all), sorted
by score desc, then grouped by kind in fixed order (Workspace, Projects,
Repositories, Directory), capped ~6/group with a "+N more" line. Matched
indices highlight chars in the label (bold/brand).

### Interaction
- Input drives results live (no debounce except the `browseDir` call).
- Keyboard: ↑/↓ move across the flat ordered (post-group) list, Enter selects
  the active item (calls `onChange`, sets input to the item's label), Esc
  clears focus/closes the list.
- Mouse: click selects. Selecting fills the input with the chosen label and
  shows the picked item as the current value.
- Colors reuse existing palette tokens: Projects emerald, Repositories violet,
  Directory slate, Workspace neutral — via the established
  `ring-1 px-1.5 py-0.5 text-[11px]` chip idiom.

## Wiring

- `LauncherSheet.tsx`: remove the `targetMode` toggle + `target`/`workingDir`
  state + the `DirectoryPicker`/`Combobox` target block; add
  `const [target, setTarget] = useState<TargetItem>(WORKSPACE_ITEM)` and render
  `<TargetPicker value={target} onChange={setTarget} disabled={launching} />`.
  In `onLaunch`, send `target: target.resolved.target`,
  `working_dir: target.resolved.working_dir`.
- `AgentLauncherSheet.tsx`: same; `buildAgentLaunch` takes the `TargetItem` and
  reads `resolved`.
- Remove `DirectoryPicker.tsx` and `Combobox.tsx` + their tests **iff** they
  become unused after this change (grep first). `repoToOption` (in
  LauncherSheet) and `getRepos` 501-tolerance stay.

No backend changes (`getRepos`/`getProjects`/`browseDir` already exist).

## Testing (vitest)
- `lib/fuzzy.ts`: ranking order (exact > subsequence; word-boundary bonus) and
  matched indices; no-match → null; empty query handling.
- `buildTargetItems`: projects/repos/dirs → correct `TargetItem.resolved`
  fields; Workspace always present/first.
- free-text kind inference: `github:o/r` → repo target; `/abs/path` & `./rel`
  & `~/x` → directory working_dir; empty → workspace.
- selection → resolved `{target|working_dir}` correctness (the launch payload).
- component smoke (jsdom): renders groups, keyboard select sets value; stub
  `getProjects`/`getRepos`/`browseDir`.

## Out of scope
- Issues/PRs as targets (needs a new CP issues API).
- A global Cmd-K palette (this picker is scoped to the Run modals).
- Backend changes.
