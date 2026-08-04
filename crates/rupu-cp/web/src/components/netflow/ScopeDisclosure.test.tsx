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
import { NetflowScopeDisclosure, netflowCoverageList, type NetflowScope } from './ScopeDisclosure';

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
