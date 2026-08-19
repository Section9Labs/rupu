// @vitest-environment jsdom
// Fix 2's entire value is textual — a coverage claim that doesn't match
// what the API can actually return. Round 3 shipped it with no direct test
// at all (the three page-level tests only assert substrings unaffected by
// the coverage list itself: "bash subprocess", "git2|object.store|
// WebSocket"). Round 4 tightened the list twice more (dropped "the
// updater" entirely; scoped "CP fleet traffic" to global only) — this file
// is the regression guard so a future copy edit can't silently reintroduce
// either defect.
//
// netflow-per-run plan, Task 8: "CP fleet traffic" came out entirely —
// `Origin::Cp` is wired to a `NullSink` (`HttpHostConnector`,
// `crates/rupu-cp/src/host/http.rs`) and is recorded nowhere, at any
// scope, so the global-only carve-out below is gone too. Every assertion
// that used to check "global scope claims it, the other two don't" now
// checks "no scope claims it."
//
// Same file, same pass, on review: "ASN refresh" turned out to be the
// identical defect — `cmd/cp.rs`'s sweep AND `maybe_refresh_asn` both
// build their download client with `NullSink` too, so it is equally
// unrecorded at every scope, global included. Its assertions below were
// updated the same way, not left as the one example that still claims
// global-only visibility.

import '@testing-library/jest-dom/vitest';
import { render, screen, cleanup } from '@testing-library/react';
import { afterEach, describe, expect, it } from 'vitest';
import {
  NetflowScopeDisclosure,
  netflowCoverageList,
  netflowEmptyStateHint,
  netflowSystemSourceHint,
  type NetflowScope,
} from './ScopeDisclosure';

afterEach(() => {
  cleanup();
});

const SCOPES: NetflowScope[] = ['run', 'project', 'global'];

describe('netflowCoverageList', () => {
  it.each(SCOPES)('never claims CP fleet traffic at %s scope', (scope) => {
    // `Origin::Cp` (`HttpHostConnector`'s fleet HTTP traffic) is wired to
    // `NullSink` and recorded nowhere — not even at global scope anymore,
    // now that the CP daemon no longer installs a persistent process-wide
    // ledger at startup. No scope may claim it.
    expect(netflowCoverageList(scope)).not.toMatch(/CP fleet traffic/i);
  });

  it.each(SCOPES)('never claims the updater at %s scope', (scope) => {
    // Blocker 2: `rupu update`'s release download installs no netflow
    // sink, and the only `Origin::Update` flow that can land races the
    // passive update-notice check in `lib.rs` — not a claim worth making
    // at any scope.
    expect(netflowCoverageList(scope)).not.toMatch(/updater/i);
  });

  it.each(SCOPES)('never claims MCP or webhook coverage at %s scope', (scope) => {
    expect(netflowCoverageList(scope)).not.toMatch(/MCP/i);
    expect(netflowCoverageList(scope)).not.toMatch(/webhook/i);
  });
});

describe('netflowEmptyStateHint', () => {
  // FIXED (previously Blocker 1, whole-branch review): `get_project_netflow`
  // now recovers a project's own runs whose ledgers fell back to the
  // global directory (`project_scoped_flows_and_dropped`), on top of
  // `rupu init` creating `.rupu/netflow/` for new projects
  // (`cmd/init.rs`'s `ensure_netflow_dir`) — so an empty project scope no
  // longer means "check global scope instead". Only project scope's hint
  // still names "global scope" at all, but now to explain WHAT the empty
  // result already covers (both its own directory and the recovered
  // fallback), not to send the operator elsewhere; run and global scope
  // have no such distinction to draw.
  it.each(SCOPES)('names the global-scope fallback only at project scope, not %s', (scope) => {
    if (scope === 'project') {
      expect(netflowEmptyStateHint(scope)).toMatch(/global scope/i);
    } else {
      expect(netflowEmptyStateHint(scope)).not.toMatch(/global scope/i);
    }
  });
});

describe('netflowSystemSourceHint', () => {
  it.each(SCOPES)('never claims CP fleet traffic at %s scope', (scope) => {
    // Task 8: "CP fleet traffic" is gone from this example list
    // entirely — see the file header note above.
    expect(netflowSystemSourceHint(scope)).not.toMatch(/CP fleet traffic/i);
  });

  it.each(SCOPES)('never claims ASN refresh at %s scope', (scope) => {
    // `Origin::System` ASN-refresh downloads are wired to `NullSink` at
    // both call sites (`cmd/cp.rs`'s sweep, this crate's own
    // `maybe_refresh_asn`) — recorded nowhere, at any scope, global
    // included. Previously (round 5) this was claimed at global scope
    // only; that carve-out is gone along with the claim.
    expect(netflowSystemSourceHint(scope)).not.toMatch(/ASN refresh/i);
  });

  it.each(SCOPES)('no longer claims a `system` fallback source at %s scope', (scope) => {
    // Finding 4, whole-branch review: `graph_view` used to derive the
    // source id from `f.ctx.run_id`, which no production `FlowCtx` ever
    // populates, so every graph collapsed to one node literally called
    // `system`. The read side now passes the owning run id in explicitly
    // (the ledger file's own name), so every source is a real run id and
    // there is no more "unattributed" fallback case to name here.
    expect(netflowSystemSourceHint(scope)).not.toMatch(/system for unattributed egress/i);
    expect(netflowSystemSourceHint(scope)).toMatch(/runs/i);
  });
});

describe('NetflowScopeDisclosure sub-agent note', () => {
  // Run scope's flows/dropped_total now fold in every sub-agent this run
  // dispatched, at any depth (`RunStore::sub_run_ids_recursive`) — the
  // operator-facing text must say so on THIS surface, the one place every
  // netflow scope-limit sentence is authored, or "this run's netflow"
  // silently reads as "only this run's own provider calls" while actually
  // including everything its sub-agents did too.
  it('names sub-agent dispatch at run scope', () => {
    render(<NetflowScopeDisclosure scope="run" />);
    expect(screen.getByText(/sub-agents/i)).toBeInTheDocument();
  });

  it.each(['project', 'global'] as NetflowScope[])(
    'does not repeat the run-scope sub-agent note at %s scope',
    (scope) => {
      // Project/global scope already union every contributing run's own
      // ledger directory-wide (`read_all_run_ledgers_in_dir`) — a
      // sub-agent's ledger file is just one more file in that union, with
      // no separate "folded from a parent" wrinkle to call out there the
      // way run scope's id-driven fold needs explaining.
      render(<NetflowScopeDisclosure scope={scope} />);
      expect(screen.queryByText(/sub-agents/i)).not.toBeInTheDocument();
    },
  );
});

describe('NetflowScopeDisclosure', () => {
  it.each(SCOPES)('never claims MCP or webhook coverage at %s scope', (scope) => {
    render(<NetflowScopeDisclosure scope={scope} />);
    expect(screen.queryByText(/MCP/i)).not.toBeInTheDocument();
    expect(screen.queryByText(/webhook/i)).not.toBeInTheDocument();
  });

  it.each(SCOPES)('never claims updater coverage at %s scope', (scope) => {
    render(<NetflowScopeDisclosure scope={scope} />);
    expect(screen.queryByText(/updater/i)).not.toBeInTheDocument();
  });

  it.each(SCOPES)('never claims CP fleet traffic at %s scope', (scope) => {
    // Task 8: the pointer sentence ("CP fleet traffic appears on the
    // global Network page only") is gone along with the claim it used to
    // qualify — `Origin::Cp` is recorded nowhere, so there is no page to
    // point a project/run viewer at anymore.
    render(<NetflowScopeDisclosure scope={scope} />);
    expect(screen.queryByText(/CP fleet traffic/i)).not.toBeInTheDocument();
    expect(screen.queryByText(/global Network page only/i)).not.toBeInTheDocument();
  });
});
