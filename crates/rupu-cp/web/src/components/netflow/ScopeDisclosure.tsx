// The ONE place every netflow scope-limit sentence in the CP is authored.
// Three surfaces render <NetflowScopeDisclosure /> verbatim — the global
// Netflow page, the project Network tab, and the run Network tab (Fix 3,
// netflow Plan 3 review round 3: the run surface previously disclosed
// nothing at all). NetflowTable's own empty-state hint quotes
// NETFLOW_COVERAGE_LIST rather than restating the list by hand, so the
// covered-surface list can't drift out of sync the way it did before this
// fix (three copy-pasted paragraphs, one of them wrong).
//
// Coverage list intentionally excludes MCP and webhooks (Fix 2): grep the
// workspace and `Origin::Mcp` / `Origin::Webhook` are constructed nowhere —
// see `rupu_netflow::ctx::Origin`'s variants and
// `crates/rupu-cp/web/src/lib/netflow.ts`'s `Origin` doc comment. Claiming
// coverage that cannot exist is exactly the failure this subsystem was
// built to prevent. Add them back here the day something actually
// constructs one, not before.
//
// The updater and CP fleet traffic ARE real, but only reachable through the
// global scope's ledger union (Fix 1) — this component doesn't vary its
// wording per scope because a viewer on the project or run surface still
// needs to know those categories exist system-wide, even though this
// particular scope won't show them; scope-specific silence about a real
// category would be its own honesty gap.

export const NETFLOW_COVERAGE_LIST =
  'provider APIs, SCM connectors, the updater, and CP fleet traffic';

export function NetflowScopeDisclosure({ className = '' }: { className?: string }) {
  return (
    <p className={['text-note text-ink-mute max-w-2xl', className].filter(Boolean).join(' ')}>
      This covers rupu&apos;s own egress — {NETFLOW_COVERAGE_LIST} — never traffic from the
      agent&apos;s bash subprocess. It also can&apos;t see non-HTTP egress: git2 clones (often a
      run&apos;s largest byte volume), object_store bucket traffic, and the node WebSocket are
      invisible here too.
    </p>
  );
}

export default NetflowScopeDisclosure;
