# Customers — Plan 2B: CP web (picker, Customers pages, assignment, launch preview) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The CP web shows and manages customers exactly as the approved mockup does: a CUSTOMER picker that scopes Dashboard, runs, sessions, usage, findings and projects; a Customers list and detail (Projects / Runs / Findings / Usage / Config tabs); assignment from the project header; and a "who this run bills" panel in the launchers.

**Architecture:** One `CustomerScopeProvider` (React context, persisted to `localStorage`, adopts a `?customer=` deep link) mounted above both shells. Every list/aggregate fetch passes the scope as `customer` to the Plan 2A endpoints; the per-host engine already turns a host's 501 into an "unavailable" slice, which a shared banner reads. New pages live under `src/pages/customers/` and `src/components/customers/`, built from the existing primitives (`SortableTable`, `TabBar`, `Badge`, `Chip`, `Segmented`, `Button`, `Input`, the config-editor tab bodies). No new dependencies.

**Tech Stack:** React 18 + TypeScript, react-router v6, Tailwind with the CP's CSS-variable tokens (`bg-panel`, `border-border`, `text-ink`, `text-ink-dim`, `text-ink-mute`, `bg-brand-50`, `text-brand-700`, `bg-surface`, …), `lucide-react` icons, vitest + @testing-library/react (jsdom per file), API mocked with `vi.spyOn(api, …)`.

**Spec:** `docs/superpowers/specs/2026-10-06-rupu-customers-design.md` §4. **Approved mockup:** https://claude.ai/artifact/88UtngsmAaJ6Nci6drCzfe (artboards: Customers list, Customer — projects tab, Customer — config layer, Customer scope picker → filtered Dashboard, Project — assign to customer, Launcher — who this run bills). **Backend:** Plan 2A (`docs/superpowers/plans/2026-10-06-rupu-customers-plan-2a-cp-backend.md`) must be complete first — this plan only consumes its routes.

## Global Constraints

- Match the mockup's layout and copy; build it from the CP's existing tokens and components — never hard-code the mockup's hex values (they were sampled from these tokens). Customer colors come ONLY from the API's `tint` (`{light, dark}`), chosen by the current theme (`useThemeMode`).
- Provenance chip colors: reuse `components/settings/ConfigField.tsx` `SOURCE_CLASS`; replace Plan 1's placeholder `customer` entry with the brand-tinted style (`bg-brand-50 text-brand-700 border-brand-100`) the mockup shows.
- Accessibility as the mockup draws it: real `<button>`/`<a>`/`<input>`+`<label>`; `aria-label` on icon-only buttons; menus `role="menu"`/`menuitem`, Escape closes, focus returns to the trigger.
- Never block a page on one slow host (per-host engine semantics unchanged).
- A scope the backend rejects (400 unknown/malformed slug, e.g. a deleted customer still in `localStorage`) clears the scope and shows a one-line notice; never an empty page with no explanation.
- Web tests: co-located `*.test.tsx`, `// @vitest-environment jsdom` header, `vi.spyOn(api, …)`, `MemoryRouter`. Run with `cd crates/rupu-cp/web && npx vitest run <pattern>`; type-check with `npx tsc --noEmit`.
- `make cp-web` rebuilds `web/dist`, which release builds embed (do it once at the end; never commit `dist` unless the repo already tracks it — check `git ls-files crates/rupu-cp/web/dist | head`).
- Never `git stash`; never push; never amend.

## Alignment with the shipped Plan 2A API (binding; supersedes task text where they differ)

Plan 2A shipped after this plan was written. `docs/cp-customers-api.md` is the contract; read it before every task. Differences every task must honour:

1. **Types already exist** in `web/src/lib/api.ts`: `TintDto`, `CustomerRef`, `PreviewBody`, `ManifestEntry` (with `auth_mode`), `PreviewResponse` (with optional `host`), `api.launchPreview`, `ConfigView.{raw_customer, customer, customer_lock, layer_error}`, optional `customer` / `customer_derived` on run/agent-run/session/project/finding rows, `pricing_error?` on `UsageSummary`, `hosts_without_customer?` on usage/dashboard. Task 1 adds only what is missing (customers CRUD client, `CustomerRow` incl. `layer_error` and `rollup.hosts_without_customer`, `CustomerDetail` incl. `layer_error`, the `customer` list param).
2. **Three customer states on a row**: `customer: "<slug>"` (with `customer_derived`), `customer: null` = no customer, key ABSENT = can't say. `CustomerChip` gets a third, muted "unknown" state (`title="This host or record can't say whose this is"`); never render an absent key as "No customer".
3. **The 501 reason text** for hosts that can't filter is "can't report a customer for every run" (lists) and the aggregates' 501 for remote hosts under a filter; `HostsWithoutCustomerBanner` matches on the HTTP status + those reasons, and ALSO reads `hosts_without_customer` arrays (usage, dashboard, customer rollups) and the `X-Rupu-Hosts-Without-Customer` header where a page uses a fan-out request.
4. **`pricing_error`**: wherever a cost from a `UsageSummary` is shown (customer rows/tiles, project rows, run/session rows and details, usage headline/timeline/runs/outliers, dashboard), show a small warn marker with the `pricing_error` text as tooltip (a shared `PricingErrorMark` component, added in Task 2). Task 7 wires it everywhere the scope touches; Tasks 4–6 use it on the new pages.
5. **Reserved slug**: `CustomerFormDialog` rejects `none` inline ("`none` is reserved — it's the filter's “no customer”"), matching the API's 400.
6. **Unassign** answers 404 when the project is not assigned to that slug — show "Already unassigned" and refetch.
7. **Launch preview**: 409 (dangling assignment, unloadable config, malformed agent files) disables Launch with the server message; 400 for a bad name; show `auth_mode` next to provider/fallback accounts when present; show `host` + its warning when echoed.
8. **Config**: `PUT /api/config/customer/:slug` 400 when the layer breaks the merged config or sets a globally locked key — show the message inline; `layer_error` on `GET` → banner + open Raw (Task 8). ALSO surface `ConfigView.layer_error` on the existing project config tab (`components/project/ProjectConfigTab.tsx`) with the same banner (carried from Plan 2A).

## File map

| File | Change |
|---|---|
| `web/src/lib/api.ts` | customers client methods + `customer` params (types 2A added) |
| `web/src/lib/customerScope.tsx` | **new** — provider + `useCustomerScope()` |
| `web/src/components/customers/CustomerDot.tsx`, `CustomerChip.tsx`, `CustomerPicker.tsx`, `ScopeChip.tsx`, `HostsWithoutCustomerBanner.tsx`, `CustomerFormDialog.tsx`, `AssignProjectDialog.tsx`, `ProjectCustomerMenu.tsx`, `LaunchBillingPanel.tsx` | **new** |
| `web/src/pages/customers/Customers.tsx`, `CustomerDetail.tsx`, `CustomerConfigTab.tsx` | **new** |
| `web/src/App.tsx`, `lib/sidebarNav.ts`, `components/Layout.tsx`, `components/v2/Shell.tsx` | routes, nav, picker mount, provider |
| `web/src/lib/dashboard/useDashboardData.ts`, `lib/usage/useUsageData.ts`, `pages/runs/*.tsx`, `pages/Sessions.tsx`, `pages/Findings.tsx`, `pages/Projects.tsx`, `pages/Usage.tsx`, `pages/Dashboard.tsx` | scope wiring |
| `web/src/pages/ProjectDetail.tsx`, `components/LauncherSheet.tsx`, `components/AgentLauncherSheet.tsx`, `pages/RunDetail.tsx`, `components/settings/ConfigField.tsx` | chip, menu, billing panel |

All paths below are relative to `crates/rupu-cp/`.

---

### Task 1: API client

**Files:** Modify `web/src/lib/api.ts`. Test: `web/src/lib/api.customers.test.ts`.

**Interfaces (produce):**

```ts
export interface TintDto { light: string; dark: string }
export interface CustomerRef { slug: string; name: string; tint: TintDto; archived: boolean }
export interface CustomerDto { slug: string; name: string; notes: string | null; contact: string | null; color: string | null; tint: TintDto; archived: boolean; created_at: string }
export interface CustomerRollup { projects: number; run_count: number; usage: UsageSummary; findings_open: number; last_active: string | null }
export interface DefaultAccount { account: string; locked_by: 'global' | 'customer' | null; inherited: boolean }
export interface CustomerRow extends CustomerDto { rollup: CustomerRollup; default_account: DefaultAccount | null }
export interface CustomerDetail { customer: CustomerDto; rollup: CustomerRollup; projects: ProjectRow[]; default_account: DefaultAccount | null; layer_error: string | null }
export type CustomerScope = string | 'none' | null;   // slug | unassigned | all
export interface NewCustomerBody { slug: string; name: string; notes?: string; contact?: string; color?: string }
export interface CustomerPatch { name?: string; notes?: string; contact?: string; color?: string }   // '' clears
export interface CustomerConflict { error: string; projects: { ws_id: string; path: string }[] }

// on `api`:
getCustomers(opts?: { archived?: boolean; range?: '7d' | '30d' | 'all' }): Promise<CustomerRow[]>;
getCustomer(slug: string, range?: '7d' | '30d' | 'all'): Promise<CustomerDetail>;
createCustomer(body: NewCustomerBody): Promise<CustomerDto>;
updateCustomer(slug: string, patch: CustomerPatch): Promise<CustomerDto>;
archiveCustomer(slug: string, archived: boolean): Promise<CustomerDto>;
deleteCustomer(slug: string): Promise<void>;                 // throws ApiError(409) with body CustomerConflict
assignProject(slug: string, wsId: string): Promise<ProjectRow>;
unassignProject(slug: string, wsId: string): Promise<void>;
getCustomerConfig(slug: string): Promise<ConfigView>;        // GET /api/config?customer=
putCustomerConfig(slug: string, body: { raw?: string; patch?: Record<string, unknown> }): Promise<void>;
```

…and an optional `customer?: CustomerScope` on the params of `getRuns`, `getWorkflowRuns`, `getAgentRuns`, `getAutoflowRuns` (if it hits a runs endpoint), `getSessions`, `getFindings`, `getProjects`, `getUsage`, `getUsageTimeline`, `getUsageRuns`, `getDashboard` — appended as `customer=<slug|none>` only when non-null (so every existing call is byte-identical). Check which of these Plan 2A Task 6–8 already added types for; add only what is missing.

- [ ] **Step 1: Failing test** — stub `fetch` (the `api.scope.test.ts` pattern) and assert: `getCustomers({archived:true, range:'7d'})` hits `/api/customers?archived=1&range=7d`; `deleteCustomer` on a 409 rejects with an `ApiError` whose `status === 409` and `body.projects[0].ws_id` is set; `getWorkflowRuns({customer:'acme', …})` includes `customer=acme`; `customer: null` omits the param; `customer: 'none'` sends `customer=none`.
- [ ] **Step 2:** `npx vitest run src/lib/api.customers.test.ts` → fails.
- [ ] **Step 3:** Implement with the file's existing `request<T>` / `URLSearchParams` conventions (check how `ApiError` exposes the parsed body — `api.ts:27` — and use it for the 409).
- [ ] **Step 4:** test passes; `npx tsc --noEmit` clean. **Commit** `feat(cp-web): customers API client`.

---

### Task 2: Customer scope, dot and chip

**Files:** Create `web/src/lib/customerScope.tsx`, `web/src/components/customers/CustomerDot.tsx`, `web/src/components/customers/CustomerChip.tsx`. Modify `web/src/App.tsx` (wrap `<AppRoutes>` in `<CustomerScopeProvider>` inside `<BrowserRouter>`). Tests: `customerScope.test.tsx`, `CustomerChip.test.tsx`.

**Interfaces (produce):**

```ts
export const CUSTOMER_SCOPE_KEY = 'rupu.cp.customer';
export interface CustomerScopeValue {
  scope: CustomerScope;                 // null = all customers
  customer: CustomerRow | null;         // the scoped customer's row (null for all / none / not loaded)
  customers: CustomerRow[];             // active customers (archived loaded on demand by the picker)
  setScope(next: CustomerScope): void;
  reload(): void;                       // after create/rename/archive/delete
  notice: string | null;                // e.g. "Customer “acme” no longer exists — showing all customers."
}
export function CustomerScopeProvider({ children }: { children: ReactNode }): JSX.Element;
export function useCustomerScope(): CustomerScopeValue;
export function useCustomerParam(): CustomerScope;   // the value list fetches pass as `customer`
```

Behaviour: initial scope = `?customer=` from the URL when present (adopted, then persisted), else `localStorage`, else `null`; `setScope` persists (try/catch around storage, like `shellState.tsx`). Loads `api.getCustomers()` once on mount (and on `reload()`); when the stored slug is not in the list (and not `'none'`), clear it and set `notice`. A failed `getCustomers` keeps the scope (pages still pass it and the backend validates it) and exposes `customers: []`.

`CustomerDot({ tint, size = 8, title })` — a round span colored `tint.dark` / `tint.light` by `useThemeMode()` (see `components/codename/CrewChip.tsx` for the pattern); `aria-hidden` unless `title`.
`CustomerChip({ customer: CustomerRef | null, derived?: boolean, onRemove?, size?: 'sm' | 'md' })` — pill with dot + name in the brand-tinted style; `null` renders an outlined, dashed "No customer" pill (the mockup's project-header state); `derived` adds a muted "derived" tooltip (`title="Attributed from the project's current customer"`); `onRemove` renders an `aria-label="Clear customer scope"` × button.

- [ ] **Step 1: Failing tests** — provider: adopts `?customer=acme` from `MemoryRouter initialEntries`; persists to `localStorage` on `setScope`; a stored slug missing from `getCustomers` clears scope and sets `notice`; `useCustomerParam` returns the scope. Chip: renders name + dot color per theme; `null` renders "No customer"; × calls `onRemove`.
- [ ] **Step 2 → 4:** fail, implement, pass; `tsc`. **Commit** `feat(cp-web): customer scope provider, dot and chip`.

---

### Task 3: Picker, nav and routes

**Files:** Create `web/src/components/customers/CustomerPicker.tsx`. Modify `web/src/components/Layout.tsx` (picker block under the brand link — the mockup's "CUSTOMER" label + button, `px-3 pt-3`), `web/src/components/v2/Shell.tsx` (the same picker in the top bar, left of the project `ScopeSelect`, compact variant), `web/src/lib/sidebarNav.ts` (v1 leaf `{ to: '/customers', label: 'Customers', icon: Building2, enabled: true }` after Projects; v2 `{ to: '/customers', label: 'Customers', icon: Building2 }` after Projects), `web/src/App.tsx` (routes `/customers`, `/customers/:slug`, `/customers/:slug/:tab` — static before wildcard, lazy pages). Test: `CustomerPicker.test.tsx`, extend `components/v2/Shell.test.tsx` and the Layout/sidebar test if one asserts the leaf list.

Picker (mockup artboard 4): trigger shows dot + scope name ("All customers" / customer name / "Unassigned"), brand-tinted border when scoped. Popover: search input ("Find a customer…"), rows "All customers" (total project count from the rows' `rollup.projects` sum + unassigned count if known — else omit the count), each customer (dot, name, project count, ✓ on the current), "Unassigned"; footer: "Show archived" checkbox (loads `getCustomers({archived:true})` lazily and lists archived ones muted) and "Manage →" (`/customers`). Keyboard: ↑/↓ moves, Enter picks, Escape closes; picking closes and calls `setScope`.

- [ ] **Step 1: Failing tests** — opening lists customers from the provider; typing filters; picking Acme calls `setScope('acme')` and the trigger shows "Acme Corp"; "Unassigned" sets `'none'`; Escape closes and focus returns to the trigger; "Show archived" fetches with `archived: true`. Shell test: the v2 top bar renders the picker; nav includes Customers in both shells.
- [ ] **Step 2 → 4.** **Commit** `feat(cp-web): customer picker in both shells; Customers nav and routes`.

---

### Task 4: Customers list page (artboard 1)

**Files:** Create `web/src/pages/customers/Customers.tsx`, `web/src/components/customers/CustomerFormDialog.tsx`. Test: `Customers.test.tsx`, `CustomerFormDialog.test.tsx`.

Layout (mockup): `p-8` page; header `h1 "Customers"` + subtitle "Who your projects belong to — each customer has its own config layer, accounts and spend." + primary `Button` "New customer" (opens `CustomerFormDialog`). Four stat tiles (reuse the Dashboard/ProjectDetail tile component if one exists — `grep -rn "Rollup\|StatTile\|KeyPointTiles" src/components | head`; else the `ProjectDetail` tile markup): CUSTOMERS (count, "N archived"), PROJECTS ASSIGNED ("assigned / total", "N unassigned" in warn tone — totals from `getProjects()`), COST · <range> (sum of `rollup.usage` cost, "X% from <top>"), OPEN FINDINGS (sum, "across N customers"). Toolbar: `SearchInput` "Filter customers…", `Segmented` Active/Archived/All, `Segmented` 7d/30d/all. `SortableTable<CustomerRow>` columns: CUSTOMER (dot + bold name + mono slug, links `/customers/:slug`), DEFAULT ACCOUNT (mono chip, lock icon when `locked_by`, "inherits global" muted when `inherited`), PROJECTS, RUNS, COST, OPEN FINDINGS (err tone when > 0), LAST ACTIVE (relative; default sort desc). Footer row inside the table panel: "N projects have no customer — they run on the global config." + link "Review unassigned →" (`/projects?customer=none` — sets scope `none` and navigates to Projects). `EmptyState` when no customers ("No customers yet" + New customer button). `ErrorBanner` on load failure.

`CustomerFormDialog({ mode: 'create' | 'edit', initial?, onSaved })`: fields Name (required), Slug (create only; auto-suggested from the name: lowercase, non-alnum runs → `-`, trimmed; validated `^[a-z0-9][a-z0-9-]{0,62}$` inline), Contact, Notes (textarea), Color (optional; swatches derived nothing — a text input `#rrggbb` with a preview dot; empty = "derived from slug"). Submit → `createCustomer`/`updateCustomer`; 409 "already exists" shown inline on Slug; 400 shown inline; success → `onSaved(dto)` and `useCustomerScope().reload()`.

- [ ] **Step 1: Failing tests** — renders rows from `getCustomers` (mock 3 rows like the mockup's Acme/Globex/Initech); default-account cell shows lock for `locked_by`, "inherits global" for `inherited`; segmented Archived refetches with `archived: true`; range refetches with `range`; filter narrows rows; footer link sets scope `none`; dialog: slug auto-suggest, invalid slug blocks submit, 409 shows inline, success reloads.
- [ ] **Step 2 → 4.** **Commit** `feat(cp-web): Customers list page and create/edit dialog`.

---

### Task 5: Customer detail — header, tiles, Projects tab, assignment (artboard 2)

**Files:** Create `web/src/pages/customers/CustomerDetail.tsx`, `web/src/components/customers/AssignProjectDialog.tsx`. Test: `CustomerDetail.test.tsx`, `AssignProjectDialog.test.tsx`.

Routes: `/customers/:slug` (tab `overview`), `/customers/:slug/:tab` with tabs `overview | projects | runs | findings | usage | config` (unknown tab → `overview`). Breadcrumb "Customers / <name>". Header card: dot + `h1` name + mono slug chip; meta row contact · "created <date>" · mono layer path `~/.rupu/customers/<slug>/config.toml`; notes paragraph; buttons "Edit details" (CustomerFormDialog edit) and "Archive"/"Unarchive" (confirm-free; archived customers show a muted "Archived" badge). Delete lives in a kebab ("Delete customer…") → confirm dialog; on 409 the dialog lists `projects[]` with an "Unassign all and delete" secondary action (calls `unassignProject` for each, then `deleteCustomer`) — explicit, never automatic. Five tiles: PROJECTS, RUNS · <range> ("N running" from `getRuns({customer, lifecycle:'active'})` count — if that costs another call, show the rollup only), FINDINGS ("needs attention" err tone when > 0), COST · <range> (tokens sub-line), BILLS TO (mono account; sub-line "locked by customer" | "customer default" | "inherits global"). `layer_error` → an `ErrorBanner` "This customer's config layer doesn't parse — runs of its projects will fail until it's fixed." with a link to the Config tab.

Projects tab: "N projects · runs in their subdirectories count too" + primary "Assign project" → `AssignProjectDialog` (lists `getProjects()` rows whose `customer` is null — plus a "Show projects of other customers" toggle that lists the rest with their current chip; picking one assigned elsewhere shows "Moves <project> from <other> to <this>" and requires a second click); `SortableTable<ProjectRow>` with the Projects page's columns (reuse `PROJECT_COLUMNS` from `pages/Projects.tsx` — export it if needed) plus an actions column "Unassign" (`unassignProject` → refetch). Overview tab: the Projects tab's cost-by-project bars (mockup artboard 4's "COST BY PROJECT" panel, from `detail.projects[].usage`) + "Recent runs" (`getRuns({customer: slug, limit: 10})` rendered with the existing runs table row component).

- [ ] **Step 1: Failing tests** — header + tiles from a mocked `getCustomer`; BILLS TO sub-line per `default_account`; tabs route; Unassign calls the API and refetches; Assign dialog lists unassigned projects, assigning calls `assignProject`; moving a project assigned elsewhere needs the confirm click; delete 409 lists projects and "Unassign all and delete" runs the sequence; `layer_error` banner shows.
- [ ] **Step 2 → 4.** **Commit** `feat(cp-web): customer detail — header, tiles, projects, assignment`.

---

### Task 6: Customer Runs / Findings / Usage tabs

**Files:** Modify `web/src/pages/customers/CustomerDetail.tsx`; reuse list components. Test: extend `CustomerDetail.test.tsx`.

- Runs tab: the Workflow/Agent runs tables (the components behind `pages/runs/WorkflowRuns.tsx` / `AgentRuns.tsx` — if they are page components, extract their table+fetch into a reusable `RunsList({ kind, customer })` without changing the pages' behaviour) with `customer` fixed to the slug, via the per-host engine (so remote hosts that can't filter show as unavailable).
- Findings tab: the findings list (`pages/Findings.tsx`'s table) with `customer`.
- Usage tab: `useUsageData` with `customer` (Task 7 adds the param), rendering the Usage page's headline + breakdown components.

- [ ] **Step 1: Failing tests** — each tab calls its API with `customer: '<slug>'`.
- [ ] **Step 2 → 4.** **Commit** `feat(cp-web): customer runs, findings and usage tabs`.

---

### Task 7: The scope filters the CP

**Files:** Modify `web/src/lib/dashboard/useDashboardData.ts` (+ `customer` arg, part of the refetch key), `web/src/lib/usage/useUsageData.ts` (same), `web/src/pages/runs/WorkflowRuns.tsx`, `AgentRuns.tsx`, `AutoflowRuns.tsx`, `web/src/pages/Sessions.tsx` (fetch passes `customer: useCustomerParam()`, and it is in `deps`), `web/src/pages/Findings.tsx`, `web/src/pages/Projects.tsx` (+ CUSTOMER column with `CustomerChip`), `web/src/pages/Usage.tsx`, `web/src/pages/Dashboard.tsx` (header `ScopeChip`). The v2 pages (`ActivityV2`, `SecurityV2`) render the same page components — verify they pick the scope up through them. Create `web/src/components/customers/ScopeChip.tsx` (the mockup's header chip: dot + name + × → `setScope(null)`; renders nothing when unscoped) and `web/src/components/customers/HostsWithoutCustomerBanner.tsx`. Tests: extend each page's existing test file with one scope case; new tests for the two components.

`HostsWithoutCustomerBanner({ hosts }: { hosts: { name: string; state: string; reason: string | null }[] })` — when the scope is set and any host is `unavailable` with a reason mentioning customers (the 2A 501 text: "can't report customers"), render the mockup's warn banner: "<names> run an older rupu that can't tag runs with a customer — its runs are left out of this view, not counted as zero." Feed it from the per-host `slices` (runs/sessions pages) and from `useDashboardData`/`useUsageData` host states (Dashboard, Usage). Also show `useCustomerScope().notice` as a dismissible info line at the top of `Layout`'s `<main>` / the v2 content area.

- [ ] **Step 1: Failing tests** — with scope `acme`: Dashboard calls `getDashboard(range, host, 'acme')` per host and shows the ScopeChip; WorkflowRuns passes `customer: 'acme'`; a slice `unavailable` with the customer reason renders the banner; Projects shows the CUSTOMER column and passes `customer`; clearing the chip sets scope `null` and refetches unfiltered.
- [ ] **Step 2 → 4.** **Commit** `feat(cp-web): the customer scope filters dashboard, runs, sessions, usage, findings, projects`.

---

### Task 8: Customer Config tab (artboard 3)

**Files:** Create `web/src/pages/customers/CustomerConfigTab.tsx`. Modify `web/src/components/settings/ConfigField.tsx` (`SOURCE_CLASS.customer` → brand tint; a `locked_by`-aware lock badge: "locked by customer" brand / "locked by global policy" warn), `web/src/components/ConfigEditor.tsx` only if the tab bodies need a prop to distinguish lock owners. Test: `CustomerConfigTab.test.tsx`, extend `ConfigEditor.dotted.test.tsx` if lock keys change shape.

Model it on `components/project/ProjectConfigTab.tsx`: description line "Customer layer, resolved from `~/.rupu/customers/<slug>/config.toml`. It applies to <name>'s N projects, between the global config and each repo's `.rupu/config.toml`. A key you lock here can't be overridden by the repo." + "Save changes"; sub-tabs General / Providers / Autoflow / SCM / Issues / Pricing / Policy / Raw using the shared tab bodies with `eff`/`prov` from `getCustomerConfig(slug)`; edits `putCustomerConfig(slug, {patch})`; Raw tab edits `raw_customer` (`{raw}`). Lock toggles ("Lock for projects", the mockup's switch next to a customer-sourced field): `onToggleLock(key)` writes `patch: {"policy.lock": <customer_lock ± key>}` — keys use the dotted-key contract (`quoteSegment`), the same as Settings' global locks. Fields locked by the GLOBAL policy render read-only with the warn "locked by global policy" chip and "The global [policy].lock pins this — a customer can't change it." `layer_error` → a banner above the tabs and the Raw tab opens by default so it can be fixed. SCM routing section: the warn note "These rules replace the global [[scm.rules]] for <name>'s projects (arrays replace, they don't merge)." whenever the customer layer sets `scm.rules`. Under Default provider, when it is a customer-sourced named account: "Declared globally by `rupu auth login --account <name> --kind <vendor>`" (vendor from `eff.providers[<name>].kind`; omit the line when unknown).

- [ ] **Step 1: Failing tests** — renders provenance chips (`customer` brand, `global`); lock toggle sends the patch with the canonical dotted key; a globally locked key is read-only with the warn chip; `layer_error` shows the banner and opens Raw; saving Raw sends `{raw}`.
- [ ] **Step 2 → 4.** **Commit** `feat(cp-web): customer config tab with customer locks`.

---

### Task 9: Project header assignment (artboard 5) and run chips

**Files:** Create `web/src/components/customers/ProjectCustomerMenu.tsx`. Modify `web/src/pages/ProjectDetail.tsx` (chip + menu next to the `h1`), `web/src/pages/RunDetail.tsx` (customer chip in the header row, before the host span; `derived` styling when `customer_derived`), the runs tables' row component (a small `CustomerDot` + name column only when the scope is `null` — when scoped, every row is that customer). Tests: `ProjectCustomerMenu.test.tsx`, extend `ProjectDetail.test.tsx`, `RunDetail.test.tsx`.

Menu (mockup): trigger = `CustomerChip` (or dashed "No customer"); `role="menu"` "ASSIGN TO CUSTOMER" with each active customer (dot, name, mono default account), divider, "Unassign" (when assigned), "New customer…" (opens `CustomerFormDialog`, then assigns the new one). Hovering/focusing a customer shows the preview note: "Assigning to <name> switches this project's runs to <account> (<locked|customer default|inherits global>) …" built from that customer's `default_account` (and, when the customer layer sets `scm.rules`, "and routes <owner>/* repos to <account>" — read from `getCustomerConfig` lazily on focus; omit when not loaded). "Runs already finished keep their history." Picking assigns (`assignProject`) and updates the header chip; errors inline.

- [ ] **Step 1: Failing tests** — menu lists customers; preview text from `default_account`; assign/unassign call the API and the chip updates; "New customer…" creates then assigns; RunDetail shows the chip from the run record's `customer`, muted + tooltip when derived.
- [ ] **Step 2 → 4.** **Commit** `feat(cp-web): assign from the project header; customer chips on runs`.

---

### Task 10: Launcher billing panel (artboard 6)

**Files:** Create `web/src/components/customers/LaunchBillingPanel.tsx`. Modify `web/src/components/LauncherSheet.tsx`, `web/src/components/AgentLauncherSheet.tsx`. Tests: `LaunchBillingPanel.test.tsx`, extend both launcher tests.

`LaunchBillingPanel({ body }: { body: PreviewBody | null })` — calls `api.launchPreview(body)` (debounced 250 ms, aborted on change) whenever the definition, target/working dir, scope or host changes; renders the mockup's brand-tinted section: title "This run uses <customer>'s accounts" (or "This run uses the global accounts" when `customer` is null), one row per `accounts[]` entry grouped by role (Provider / Fallbacks / SCM) with mono account and muted `source`; `warnings[]` as warn lines; a 409 (dangling assignment) as an error line — and the launcher's Launch button is disabled while that error stands (the launch would fail the same way). The project field in the launcher shows the `CustomerChip` from `preview.customer`. No "On host …" section yet (Plan 3 adds it).

- [ ] **Step 1: Failing tests** — preview requested with the launcher's current body; renders provider/fallback/scm rows; null customer → "global accounts"; 409 disables Launch; changing the target re-requests (debounced — use fake timers).
- [ ] **Step 2 → 4.** **Commit** `feat(cp-web): launcher shows which customer and accounts a run uses`.

---

### Task 11: Build, look, and compare with the mockup

- [ ] **Step 1:** `cd crates/rupu-cp/web && npx tsc --noEmit && npx vitest run` (whole web suite — it is fast) → all pass.
- [ ] **Step 2: Seed a demo home** (scratch dir, never `~/.rupu`): build the CLI (`cargo build -p rupu-cli`), then with `RUPU_HOME=<scratch>/home`: create three customers (Acme with `default_provider = "anthropic-acme"` locked + `[[scm.rules]]`, Globex, Initech), four throwaway git dirs under `<scratch>/code/` assigned to them, and a few runs (use the mock provider: `RUPU_MOCK_PROVIDER_SCRIPT` with a one-turn script, `rupu run`/`rupu workflow run` in each dir — copy the env setup from `crates/rupu-cli/tests/serial/customer_layer.rs`).
- [ ] **Step 3: Serve it** — `make cp-web`, then add a `.claude/launch.json` entry `{"name":"cp-customers-demo","runtimeExecutable":"<repo>/target/debug/rupu","runtimeArgs":["cp","serve","--bind","127.0.0.1:7899"],"port":7899}` with `RUPU_HOME` set (check `launch.json` supports `env`; else a tiny wrapper script under the scratch dir), and open it with the built-in browser's `preview_start` by name. Do not touch the user's live CP on :7878.
- [ ] **Step 4: Screenshot each mockup artboard's screen** (Customers list, customer Projects tab, Config tab, picker open + scoped Dashboard, project assign menu, launcher) at 1440×900, dark theme, and compare with the mockup. Fix layout/copy drift in a follow-up commit; list anything deliberately different in the report.
- [ ] **Step 5:** Stop the preview server; remove the launch.json entry if it was added only for this. **Commit** any fixes `fix(cp-web): match the customers mockup`.

---

### Task 12: Docs and review

- [ ] `docs/cp-customers-api.md` (from 2A) gains a "Web" section: picker semantics (`localStorage` key, `?customer=` deep link, `none`), where customers appear. `CLAUDE.md` `rupu-cp` bullet: the web customer scope (`lib/customerScope.tsx`) and pages. `TODO.md`: mark Plan 2 done; keep Plan 3.
- [ ] `superpowers:requesting-code-review` on the branch; fix behaviour bugs in-branch. Then the user looks at the running UI before the PR merges (UI rule).
