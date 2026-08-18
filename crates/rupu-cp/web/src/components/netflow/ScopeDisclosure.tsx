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

export type NetflowScope = 'run' | 'project' | 'global';

/** Netflow's full coverage list — true at EVERY scope now (no scope adds
 *  anything on top of this one; see the header comment's "CP fleet
 *  traffic" note for why that stopped being true for global scope). Safe
 *  to use standalone (e.g. NetflowTable's empty-state hint, which has no
 *  scope of its own to key off) or through [`netflowCoverageList`], which
 *  now just returns this same string for every scope. */
export const NETFLOW_COVERAGE_LIST = 'provider APIs, SCM connectors';

/** `NETFLOW_COVERAGE_LIST`, at every scope — see the header comment's "CP
 *  fleet traffic" note for why global scope no longer adds anything on
 *  top. Kept scope-parameterized (the parameter is currently unused) so a
 *  genuinely scope-specific category can be reintroduced here later
 *  without every call site changing shape. */
export function netflowCoverageList(_scope: NetflowScope): string {
  return NETFLOW_COVERAGE_LIST;
}

function disclosureText(scope: NetflowScope): string {
  const coverage = netflowCoverageList(scope);
  return (
    `This covers rupu's own egress — ${coverage} — never traffic from the agent's bash ` +
    `subprocess. It also can't see non-HTTP egress: git2 clones (often a run's ` +
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
 *  "CP fleet traffic" came out of the example list entirely (netflow-
 *  per-run plan, Task 8) for the same reason it came out of the main
 *  disclosure — see this file's header comment. It is no longer
 *  conditional on `scope`; it simply isn't offered as an example anywhere
 *  anymore.
 *
 *  "ASN refresh" is still offered conditionally at `scope === 'global'`
 *  only (round 5's fix) — untouched by the Task 8 pass that removed "CP
 *  fleet traffic" above.
 *
 *  The `system` SOURCE ITSELF stays valid at every scope, though — that's
 *  a decision, not an oversight: `Origin::System` also covers auth/oauth
 *  token exchange (`rupu-auth`'s resolver / oauth/device / oauth/callback).
 *  With one ledger file per run (and project scope reading every ledger
 *  file under a workspace's netflow directory), that traffic reaches
 *  project and run scope because it lands directly in the relevant ledger
 *  file — attribution is by FILE, not by `ctx.run_id` — or, for a run
 *  whose own ledger file could not be opened, via that run's transcript,
 *  which the same per-run sink always writes to regardless of the
 *  ledger's fate (`merge_with_transcript` in
 *  `rupu-cp/src/api/netflow.rs`; see that module's doc). Only the
 *  PARENTHETICAL EXAMPLE is scope-gated here, not the "or system for
 *  unattributed egress" clause that names the source. */
export function netflowSystemSourceHint(scope: NetflowScope): string {
  const examples = scope === 'global' ? ' (ASN refresh)' : '';
  return `Sources — runs, or system for unattributed egress${examples} — connect to the host:port endpoints they reached.`;
}

export default NetflowScopeDisclosure;
