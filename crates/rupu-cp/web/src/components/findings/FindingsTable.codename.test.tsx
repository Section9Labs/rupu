// @vitest-environment jsdom
// Findings surfaces show the declaring agent's codename: FindingsTable (global /
// project / coverage) gets an "Agent" column; the per-run FindingRow shows it
// inline. Derived (legacy) names render muted.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect } from 'vitest';
import { render, screen, cleanup } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { FindingsTable } from './FindingsTable';
import FindingRow from './FindingRow';
import type { FindingOut, FindingRecord } from '../../lib/api';

afterEach(cleanup);

const record: FindingRecord = {
  id: 'f1', scope: null, summary: 'SQL injection', severity: 'high',
  evidence: { rationale: '' }, declared_by: null, declared_at: '2026-06-01T00:00:00Z',
};
const out: FindingOut = {
  ...record, codename: 'cobalt-harbor/heron#3', codename_derived: false,
  ws_id: 'ws', project: 'proj', target_id: 'tgt',
};

describe('FindingsTable — agent column', () => {
  it('renders the declaring agent codename with its crew', () => {
    render(<MemoryRouter><FindingsTable findings={[out]} showProvenance /></MemoryRouter>);
    expect(screen.getByRole('columnheader', { name: /agent/i })).toBeInTheDocument();
    expect(screen.getByText('heron#3')).toBeInTheDocument();
    expect(screen.getByText('cobalt-harbor')).toBeInTheDocument();
  });

  it('renders a derived codename muted', () => {
    const legacy: FindingOut = { ...out, codename: 'jade-reef', codename_derived: true };
    render(<MemoryRouter><FindingsTable findings={[legacy]} showProvenance /></MemoryRouter>);
    // Crew-only (derived) codename: the crew renders exactly once, muted.
    const all = screen.getAllByText('jade-reef');
    expect(all).toHaveLength(1);
    expect(all[0].closest('.opacity-60')).not.toBeNull();
  });

  it('falls back to declared_by.codename on a plain FindingRecord (coverage detail)', () => {
    const cov: FindingRecord = { ...record, declared_by: { codename: 'amber-fjord/owl' } };
    render(<MemoryRouter><FindingsTable findings={[cov]} wsId="ws" /></MemoryRouter>);
    expect(screen.getByText('owl')).toBeInTheDocument();
  });

  it('shows a dash when no codename is known', () => {
    render(<MemoryRouter><FindingsTable findings={[record]} wsId="ws" /></MemoryRouter>);
    expect(screen.getByRole('columnheader', { name: /agent/i })).toBeInTheDocument();
  });
});

describe('FindingRow — codename', () => {
  it('shows the declaring agent codename', () => {
    render(<MemoryRouter><FindingRow finding={out} /></MemoryRouter>);
    expect(screen.getByText('heron#3')).toBeInTheDocument();
    expect(screen.getByText('cobalt-harbor')).toBeInTheDocument();
  });
});

describe('findings — agent / provider / model', () => {
  const withIdent: FindingOut = {
    ...out,
    declared_by: { run_id: 'r', model: 'claude-sonnet-4-6', surface: 'workflow', agent: 'sec-reviewer', provider: 'anthropic' },
  };

  it('FindingsTable shows agent · provider/model next to the codename', () => {
    render(<MemoryRouter><FindingsTable findings={[withIdent]} showProvenance /></MemoryRouter>);
    expect(screen.getByText('heron#3 · sec-reviewer · anthropic/claude-sonnet-4-6')).toBeInTheDocument();
  });

  it('FindingRow shows agent · provider/model next to the codename', () => {
    render(<MemoryRouter><FindingRow finding={withIdent} /></MemoryRouter>);
    expect(screen.getByText('heron#3 · sec-reviewer · anthropic/claude-sonnet-4-6')).toBeInTheDocument();
  });

  it('an old finding (model only in declared_by) shows the model', () => {
    const old: FindingOut = { ...out, declared_by: { run_id: 'r', model: 'claude-sonnet-4-6', surface: 'workflow' } };
    render(<MemoryRouter><FindingsTable findings={[old]} showProvenance /></MemoryRouter>);
    expect(screen.getByText('heron#3 · claude-sonnet-4-6')).toBeInTheDocument();
  });
});
