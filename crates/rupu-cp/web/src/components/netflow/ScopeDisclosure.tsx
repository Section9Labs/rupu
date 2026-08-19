// The ONE place every netflow scope-limit sentence in the CP is authored.
// Three surfaces render <NetflowScopeDisclosure /> — the global Netflow
// page, the project Network tab, and the run Network tab (Fix 3, netflow
// Plan 3 review round 3: the run surface previously disclosed nothing at
// all). NetflowTable's empty-state hint and NetflowGraph's empty-state hint
// (Fix 2, review round 4) both draw from the exports here too, so the
// covered-surface list can't drift out of sync the way it did before this
// fix (four copy-pasted/hand-written fragments, several of them wrong).
//
// Coverage list intentionally excludes MCP and webhooks (Fix 2, round 3):
// `rupu_netflow::ctx::Origin` no longer even HAS an `Mcp`/`Webhook` variant
// (netflow-per-run Plan 2, Task 1) — neither subsystem makes outbound HTTP,
// so there was nothing for those variants to ever be constructed for. See
// `rupu_netflow::ctx::Origin`'s doc comment and
// `crates/rupu-cp/web/src/lib/netflow.ts`'s `Origin` doc comment. Claiming
// coverage that cannot exist is exactly the failure this subsystem was
// built to prevent. Add an entry back here only if a future subsystem
// change actually gives one of them outbound HTTP to make.
//
// It also excludes "the updater" entirely: `rupu_netflow::http::init` (the
// process-global sink installer this paragraph used to cite) was deleted
// in the netflow-per-run rewrite — `rupu-netflow/src/http/` has no `init`
// at all any more. Every remaining `rupu update` / update-
// notice call site (`GithubReleaseSource::new`, `rupu-update/src/
// github.rs`) constructs its HTTP client with `Arc::new(NullSink)`
// directly, including the passive update-notice check spawned in
// `lib.rs` — there is no `Origin::Update` flow that reaches any ledger,
// at any scope, full stop. Installing a real sink there is new
// capability work, tracked as a follow-up, not a wording fix.
//
// It ALSO excludes "CP fleet traffic" entirely now (netflow-per-run plan,
// Task 8): `Origin::Cp` flows are the CP daemon's own fleet HTTP traffic
// (`HttpHostConnector::new` / `::new_with_timeout`,
// `crates/rupu-cp/src/host/http.rs`), which is deliberately wired to
// `Arc::new(NullSink)` — daemon-lifetime, not run-scoped, so it is
// recorded nowhere, at ANY scope. This used to be claimed at
// `scope === 'global'` only (Fix 2, round 4 — Blocker 1: `Origin::Cp`
// flows lived in the CP daemon's own process-wide global ledger back
// then). That ledger is gone under the per-run model — every sink this
// plan builds is scoped to one run/turn — and nothing replaced it for
// `Origin::Cp` specifically. There is no scope left where the claim is
// true, so it comes out everywhere, with no `scope`-conditional branch
// left to qualify it.
//
// It ALSO excludes "ASN refresh" entirely now (same pass, on review):
// every `Origin::System` ASN-refresh download — `cmd/cp.rs`'s gate-sweep
// tick AND this crate's own read-triggered `maybe_refresh_asn` — builds
// its client with `Arc::new(NullSink)` too, for the identical reason as
// CP fleet traffic (a background download tied to the daemon, not to any
// run). This was ALSO claimed at `scope === 'global'` only (round 5's
// fix), on the same now-gone "cp serve installs a persistent global
// ledger at startup" premise the CP-fleet-traffic carve-out relied on.
// Same conclusion: nothing captures it at any scope, so nothing here may
// claim it either.
//
// It ALSO excludes auth/oauth token exchange entirely (Finding 3,
// whole-branch review) — including a token REFRESH firing mid-run, which
// is the one case someone might expect to be in scope since it happens
// while a run is active. It isn't: `rupu-auth`'s resolver
// (`resolver.rs`), device-code flow (`oauth/device.rs`) and callback
// server (`oauth/callback.rs`) all build their client with
// `Arc::new(NullSink)`. This is a deliberate ruling, not a gap pending
// wiring — matt's scope call for this plan was explicit: "I do not care
// about update or login", and a token exchange is login even when it
// fires mid-run. See `resolver.rs`'s own comment on `refresh_inner` for
// the same ruling stated at the call site.

export type NetflowScope = 'run' | 'project' | 'global';

/** Netflow's full coverage list — true at EVERY scope now (no scope adds
 *  anything on top of this one; see the header comment's "CP fleet
 *  traffic" and "ASN refresh" notes for why that stopped being true for
 *  global scope). Safe to use standalone (e.g. NetflowTable's empty-state
 *  hint, which has no scope of its own to key off) or through
 *  [`netflowCoverageList`], which now just returns this same string for
 *  every scope. */
export const NETFLOW_COVERAGE_LIST = 'provider APIs, SCM connectors';

/** `NETFLOW_COVERAGE_LIST`, at every scope — see the header comment's "CP
 *  fleet traffic" and "ASN refresh" notes for why global scope no longer
 *  adds anything on top. Kept scope-parameterized (the parameter is
 *  currently unused) so a genuinely scope-specific category can be
 *  reintroduced here later without every call site changing shape. */
export function netflowCoverageList(_scope: NetflowScope): string {
  return NETFLOW_COVERAGE_LIST;
}

function disclosureText(scope: NetflowScope): string {
  const coverage = netflowCoverageList(scope);
  return (
    `This covers rupu's own egress — ${coverage} — never traffic from the agent's bash ` +
    `subprocess. It also can't see non-HTTP egress: git2 clones (often a run's ` +
    `largest byte volume), object_store bucket traffic, and the node WebSocket are invisible ` +
    `here too. Ledgers are kept indefinitely unless an operator runs rupu netflow prune — ` +
    `nothing here expires or rotates on its own.`
  );
}

export function NetflowScopeDisclosure({
  scope,
  className = '',
}: {
  scope: NetflowScope;
  className?: string;
}) {
  return (
    <p className={['text-note text-ink-mute max-w-2xl', className].filter(Boolean).join(' ')}>
      {disclosureText(scope)}
    </p>
  );
}

/** NetflowTable's empty-state hint (Blocker 1, whole-branch review) --
 *  authored here for the same reason `netflowSystemSourceHint` is: a
 *  scope-specific caveat can be added without `NetflowTable` itself
 *  knowing anything about the underlying ledger-directory layout.
 *
 *  FIXED (previously a wording-only caveat pending a data-reachability
 *  follow-up): `get_project_netflow` (`rupu-cp`) used to read ONLY
 *  `<workspace>/.rupu/netflow/`, and `rupu init` never created that
 *  directory -- `templates.rs`'s `GITIGNORE_ENTRIES` listed the
 *  `.gitignore` entry for it, but `cmd/init.rs` didn't create the
 *  directory itself, so on a fresh install EVERY run's ledger landed at
 *  the global fallback root regardless of project, and project scope was
 *  empty even for projects with real, captured runs. Both halves are
 *  fixed now: `rupu init` creates `.rupu/netflow/` for every NEW project
 *  (`cmd/init.rs`'s `ensure_netflow_dir`), and `get_project_netflow`
 *  additionally runs an id-driven recovery pass over the global fallback
 *  root for EXISTING projects' own runs
 *  (`rupu-cp::api::netflow::project_scoped_flows_and_dropped`). An empty
 *  project scope therefore means what it says -- no recorded runs with
 *  network flows for this project -- with two narrow, deliberate
 *  exceptions that stay invisible everywhere except global scope: a
 *  `run_id: None` flow with no run record to attribute it to (there is
 *  nothing to enumerate it from), and a ledger whose owning run's record
 *  has since been deleted (an orphan). Neither is a bug to chase; see
 *  that function's own doc comment for the full accounting. */
export function netflowEmptyStateHint(scope: NetflowScope): string {
  const generic = `Netflow covers rupu's own egress — ${NETFLOW_COVERAGE_LIST}. It does not cover traffic from the agent's bash subprocess.`;
  if (scope !== 'project') return generic;
  return (
    `This covers both this project's own .rupu/netflow/ directory (if it has one) and ` +
    `any of its runs' ledgers that fell back to global scope instead — whether or not ` +
    `this project has that directory yet. ${generic}`
  );
}

/** NetflowGraph's empty-state hint — folded in here (Fix 2, round 4) so its
 *  source-labelling claim can't drift from the same scope rules as the
 *  rest of this file.
 *
 *  UPDATED (Finding 4, whole-branch review): the graph's source id is no
 *  longer ever the literal string `system`. `rupu_netflow::ledger::
 *  graph_view` used to derive the source from `f.ctx.run_id`, which no
 *  production `FlowCtx` has ever populated — every graph, at every scope,
 *  was a hub-and-spoke picture of one node called `system`. The read side
 *  now passes the OWNING RUN ID in explicitly (`rupu-cp/src/api/
 *  netflow.rs`'s `resolve_ledger_paths`/`read_all_run_ledgers_in_dir`
 *  already know it — it's the ledger FILE's own name), so every source
 *  node is a real run id: one per run at run scope, one per contributing
 *  run at project/global scope. There is no longer an "unattributed"
 *  fallback case in the graph endpoint's own code, so this hint no
 *  longer promises one.
 *
 *  `Origin::System` — auth/oauth token exchange (`resolver.rs`,
 *  `oauth/device.rs`, `oauth/callback.rs`), the theme-URL fetch
 *  (`output/theme.rs`), and the ASN-table refresh (`cmd/cp.rs`'s sweep,
 *  this crate's `maybe_refresh_asn`) — is unrelated to this and worth
 *  noting for a different reason: EVERY production construction site for
 *  it is wired to `Arc::new(NullSink)`, so that traffic reaches no ledger
 *  at any scope either. It was never going to produce a `system`-labelled
 *  node regardless of the `graph_view` bug above. */
export function netflowSystemSourceHint(_scope: NetflowScope): string {
  return `Sources — runs — connect to the host:port endpoints they reached.`;
}

/** `NetflowResponse.window` — the wire shape (kept structurally typed
 *  rather than imported from `lib/netflow.ts` to avoid a dependency
 *  cycle: that module doesn't need to know about this one). */
export interface NetflowWindowEcho {
  from: string | null;
  to: string | null;
}

/** Whether the server actually applied a `?from=`/`?to=` bound to produce
 *  the current result — read from the response's OWN echo
 *  (`NetflowResponse.window`), never from the picker's local UI state.
 *  Shared by `NetflowTable` and `NetflowGraph` (Critical 1/2, whole-branch
 *  review round 1) so "was a range applied" can't end up computed two
 *  different ways that drift apart. */
export function netflowWindowApplied(window?: NetflowWindowEcho | null): boolean {
  return window != null && (window.from !== null || window.to !== null);
}

/** The empty-state hint for "a `?from=`/`?to=` window was applied and
 *  matched nothing" — shared by `NetflowTable`'s per-flow empty state and
 *  `NetflowGraph`'s topology empty state (Critical 2, whole-branch review
 *  round 1: these two used to disagree on the same screen — an in-range-
 *  empty result showed the table's "No network flows in this range"
 *  directly above the graph's unqualified "No flows to graph for this
 *  scope", and only one of those two adjacent sentences could be true).
 *  Says flows "may still exist outside" rather than asserting they do —
 *  an empty windowed result is not proof the wider scope is empty, only
 *  that this window is. Names the actual clearing affordance ("All")
 *  rather than the vaguer "clear the range" (Minor finding, same round:
 *  the hint and the control it referred to used different words for the
 *  same action). */
export function netflowRangeEmptyHint(): string {
  return (
    `Flows may still exist outside the selected window — widen the range, or choose All to ` +
    `see everything recorded.`
  );
}

export default NetflowScopeDisclosure;
