// @vitest-environment jsdom
// Fix 2's entire value is textual — a coverage claim that doesn't match
// what the API can actually return. Round 3 shipped it with no direct test
// at all (the three page-level tests only assert substrings unaffected by
// the coverage list itself: "bash subprocess", "git2|object.store|
// WebSocket"). Round 4 tightened the list twice more (dropped "the
// updater" entirely; scoped "CP fleet traffic" to global only) — this file
// is the regression guard so a future copy edit can't silently reintroduce
// either defect.

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
  it('includes CP fleet traffic ONLY for global scope', () => {
    // Blocker 1: `Origin::Cp` flows live only in the CP daemon's global
    // ledger (Fix 1) — never unioned into project scope, never
    // attributable to a single run. The coverage-LIST fragment (as opposed
    // to the pointer sentence that also legitimately names "CP fleet
    // traffic" when explaining where it went) must not claim it anywhere
    // else.
    expect(netflowCoverageList('project')).not.toMatch(/CP fleet traffic/i);
    expect(netflowCoverageList('run')).not.toMatch(/CP fleet traffic/i);
    expect(netflowCoverageList('global')).toMatch(/CP fleet traffic/i);
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
  it('includes ASN refresh and CP fleet traffic ONLY for global scope', () => {
    // Round 5: the same defect as Blocker 1, one example over — every
    // `Origin::System` ASN-refresh download resolves the sink `cp serve`
    // installs at startup (always the daemon's global-only ledger); there
    // is no other `asn::refresh` call site, so it can never appear at
    // project or run scope either. Guarding both strings in one test keeps
    // them from drifting apart the way round 4 -> round 5 just showed they
    // can.
    expect(netflowSystemSourceHint('project')).not.toMatch(/ASN refresh/i);
    expect(netflowSystemSourceHint('project')).not.toMatch(/CP fleet traffic/i);
    expect(netflowSystemSourceHint('run')).not.toMatch(/ASN refresh/i);
    expect(netflowSystemSourceHint('run')).not.toMatch(/CP fleet traffic/i);
    expect(netflowSystemSourceHint('global')).toMatch(/ASN refresh/i);
    expect(netflowSystemSourceHint('global')).toMatch(/CP fleet traffic/i);
  });

  it.each(SCOPES)('still names `system` as a source at %s scope', (scope) => {
    // The scope-gated piece is the PARENTHETICAL EXAMPLE, not the "or
    // system for unattributed egress" clause itself — `Origin::System`
    // also covers auth/oauth token exchange, which genuinely can reach
    // project scope (the project's own ledger) and run scope (the run's
    // own transcript). Dropping the whole clause at those scopes would
    // overcorrect into a different inaccuracy.
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

  it('points project/run viewers at the global page for CP fleet traffic', () => {
    render(<NetflowScopeDisclosure scope="project" />);
    expect(screen.getByText(/global Network page only/i)).toBeInTheDocument();
    cleanup();

    render(<NetflowScopeDisclosure scope="run" />);
    expect(screen.getByText(/global Network page only/i)).toBeInTheDocument();
    cleanup();

    // Global scope already shows the real thing — no pointer note needed.
    render(<NetflowScopeDisclosure scope="global" />);
    expect(screen.queryByText(/global Network page only/i)).not.toBeInTheDocument();
  });
});
