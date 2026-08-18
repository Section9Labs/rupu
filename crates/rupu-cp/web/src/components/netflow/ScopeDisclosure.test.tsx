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

import '@testing-library/jest-dom/vitest';
import { render, screen, cleanup } from '@testing-library/react';
import { afterEach, describe, expect, it } from 'vitest';
import {
  NetflowScopeDisclosure,
  netflowCoverageList,
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

describe('netflowSystemSourceHint', () => {
  it('includes ASN refresh ONLY for global scope', () => {
    // Round 5: every `Origin::System` ASN-refresh download resolves a
    // sink built inline at its call site (`cmd/cp.rs`'s sweep,
    // `maybe_refresh_asn`); there is no other `asn::refresh` call site, so
    // it can never appear at project or run scope. Untouched by Task 8.
    expect(netflowSystemSourceHint('project')).not.toMatch(/ASN refresh/i);
    expect(netflowSystemSourceHint('run')).not.toMatch(/ASN refresh/i);
    expect(netflowSystemSourceHint('global')).toMatch(/ASN refresh/i);
  });

  it.each(SCOPES)('never claims CP fleet traffic at %s scope', (scope) => {
    // Task 8: "CP fleet traffic" is gone from this example list
    // entirely — see the file header note above.
    expect(netflowSystemSourceHint(scope)).not.toMatch(/CP fleet traffic/i);
  });

  it.each(SCOPES)('still names `system` as a source at %s scope', (scope) => {
    // The scope-gated piece is the PARENTHETICAL EXAMPLE, not the "or
    // system for unattributed egress" clause itself — `Origin::System`
    // also covers auth/oauth token exchange, which genuinely can reach
    // project scope (lands directly in that workspace's own ledger file)
    // and run scope (the run's own ledger file, or its transcript for a
    // run whose ledger could not be opened). Dropping the whole clause at
    // those scopes would overcorrect into a different inaccuracy.
    expect(netflowSystemSourceHint(scope)).toMatch(/system for unattributed egress/i);
  });
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
