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
// grep the workspace and `Origin::Mcp` / `Origin::Webhook` are constructed
// nowhere — see `rupu_netflow::ctx::Origin`'s variants and
// `crates/rupu-cp/web/src/lib/netflow.ts`'s `Origin` doc comment. Claiming
// coverage that cannot exist is exactly the failure this subsystem was
// built to prevent. Add them back here the day something actually
// constructs one, not before.
//
// It also excludes "the updater" entirely (Fix 2, round 4 — Blocker 2):
// there are exactly two production `rupu_netflow::http::init` call sites in
// the workspace, `cmd/run.rs` and `cmd/cp.rs`. `cmd/update.rs` (the `rupu
// update` release download) calls neither, so that traffic is wired to a
// `NullSink` and recorded nowhere at any scope. The only `Origin::Update`
// flow that CAN land is the passive update-notice check spawned in
// `lib.rs`, which races `cmd/run.rs`'s `init` call further down dispatch —
// not a claim worth making. Installing a sink in `cmd/update.rs` is new
// capability work, tracked as a follow-up, not a wording fix.
//
// `NetflowScope` gates "CP fleet traffic" (Fix 2, round 4 — Blocker 1):
// `Origin::Cp` flows live ONLY in the CP daemon's global ledger
// (`rupu_cp::api::netflow::read_all_workspaces_sync`, Fix 1) — deliberately
// never unioned into project scope, and a run's flows never carry
// `Origin::Cp` either (they're process-global, not attributable to one
// run). Asserting "this covers CP fleet traffic" on the project or run
// Network tab would claim a category the code deliberately withholds at
// that scope. `scope === 'global'` is therefore the only branch that
// includes it; the other two carry a pointer to where it actually lives.

export type NetflowScope = 'run' | 'project' | 'global';

/** The subset of netflow's coverage that is true at EVERY scope — safe to
 *  use standalone (e.g. NetflowTable's empty-state hint, which has no
 *  scope of its own to key off). Scope-aware callers should go through
 *  [`netflowCoverageList`] instead so global scope's extra category
 *  (CP fleet traffic) is represented correctly. */
export const NETFLOW_COVERAGE_LIST = 'provider APIs, SCM connectors';

/** `NETFLOW_COVERAGE_LIST`, plus CP fleet traffic when (and only when)
 *  `scope` is `'global'` — see this file's header comment (Blocker 1) for
 *  why the other two scopes must not claim it. */
export function netflowCoverageList(scope: NetflowScope): string {
  return scope === 'global' ? `${NETFLOW_COVERAGE_LIST}, and CP fleet traffic` : NETFLOW_COVERAGE_LIST;
}

function disclosureText(scope: NetflowScope): string {
  const coverage = netflowCoverageList(scope);
  const cpNote =
    scope === 'global' ? '' : ' CP fleet traffic appears on the global Network page only.';
  return (
    `This covers rupu's own egress — ${coverage} — never traffic from the agent's bash ` +
    `subprocess.${cpNote} It also can't see non-HTTP egress: git2 clones (often a run's ` +
    `largest byte volume), object_store bucket traffic, and the node WebSocket are invisible ` +
    `here too.`
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

/** NetflowGraph's empty-state hint — folded in here (Fix 2, round 4) so its
 *  "what does the `system` source cover" example list can't drift from the
 *  same scope rules as the rest of this file. Previously hard-coded in
 *  NetflowGraph.tsx itself, independently of `NETFLOW_COVERAGE_LIST`, and
 *  claimed both "updater" and "CP fleet traffic" unconditionally — the same
 *  two defects as the main disclosure, just in a fourth, un-centralized
 *  copy.
 *
 *  Round 5 fixed a THIRD instance of the same defect, one example over:
 *  "ASN refresh" is exactly as global-only as "CP fleet traffic" is. Every
 *  `Origin::System` ASN-refresh download — whether from `cmd/cp.rs`'s sweep
 *  or from this crate's own `maybe_refresh_asn` — resolves the sink
 *  `cp serve` installs at startup, which is always the daemon's
 *  global-only ledger; `get_project_netflow`/`collect_run_netflow` never
 *  read it, only `read_all_workspaces_sync` (global scope) does, and there
 *  is no other `asn::refresh` call site in the workspace. So it can never
 *  appear at project or run scope — offering it as an example there was
 *  false in exactly the way the round-4 fix was supposed to close off.
 *
 *  The `system` SOURCE ITSELF stays valid at every scope, though — that's
 *  a decision, not an oversight: `Origin::System` also covers auth/oauth
 *  token exchange (`rupu-auth`'s resolver / oauth/device / oauth/callback),
 *  which genuinely can reach project scope (the project's own ledger, no
 *  run_id filter) and run scope (the run's own transcript, which records
 *  every flow observed while the run was active regardless of
 *  `ctx.run_id`). Only the PARENTHETICAL EXAMPLE is scope-gated here, not
 *  the "or system for unattributed egress" clause that names the source. */
export function netflowSystemSourceHint(scope: NetflowScope): string {
  const examples = scope === 'global' ? ' (ASN refresh, CP fleet traffic)' : '';
  return `Sources — runs, or system for unattributed egress${examples} — connect to the host:port endpoints they reached.`;
}

export default NetflowScopeDisclosure;
