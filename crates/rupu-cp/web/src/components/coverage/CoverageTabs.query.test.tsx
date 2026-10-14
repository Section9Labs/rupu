// @vitest-environment jsdom
// The coverage detail tabs filter through the query line.
import '@testing-library/jest-dom/vitest';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { api, type AuditReport, type ConcernCoverage, type FlatCatalog } from '../../lib/api';
import { clearQuery, commitQuery } from '../query/queryTestUtils';
import CoverageGapTab from './CoverageGapTab';
import CoverageAuditTab from './CoverageAuditTab';
import CoverageCatalogTab from './CoverageCatalogTab';

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

function concern(id: string, severity: string, inScope: string[], gap: string[], findings = 0): ConcernCoverage {
  return {
    concern_id: id,
    name: `${id} name`,
    severity,
    in_scope_files: inScope,
    asserted_files: inScope.filter((f) => !gap.includes(f)),
    gap_files: gap,
    clean: 0,
    findings,
    examined: 0,
    not_applicable: 0,
  };
}

const REPORT: AuditReport = {
  target_id: 't',
  concerns: [
    concern('sqli', 'high', ['src/api/a.rs', 'src/db/b.rs'], ['src/api/a.rs', 'src/db/b.rs'], 2),
    concern('xss', 'low', ['src/api/c.rs'], ['src/api/c.rs']),
    concern('authz', 'critical', ['lib/u.rs'], []),
  ],
  files: [],
  cross_model: [],
  serendipitous: [],
  total_concerns: 3,
  complete_concerns: 1,
  total_gap_files: 3,
};

const CATALOG: FlatCatalog = {
  concerns: [
    { id: 'sqli', name: 'SQL injection', description: 'queries', severity: 'high', applicable_globs: ['**/*.rs'], min_strength: 'asserted', references: [], tags: ['owasp-a03'] },
    { id: 'xss', name: 'Cross-site scripting', description: 'html', severity: 'medium', applicable_globs: ['**/*.ts'], min_strength: 'examined', references: [], tags: [] },
  ],
  sources: { sqli: 'owasp-top-10' },
  render_modes: {},
};

const ids = () => screen.queryAllByText(/^(sqli|xss|authz)$/).map((e) => e.textContent);

function renderAt(node: React.ReactNode) {
  return render(<MemoryRouter initialEntries={['/coverage/t/gap']}>{node}</MemoryRouter>);
}

describe('Gap tab', () => {
  it('keeps the rows the query matches and lists only the gap files its file: terms keep', async () => {
    vi.spyOn(api, 'getCoverageAudit').mockResolvedValue(REPORT);
    renderAt(<CoverageGapTab target="t" />);
    await screen.findByText('sqli');
    expect(ids()).toEqual(['sqli', 'xss']);

    commitQuery('Filter gaps', 'severity>=medium file:api');
    expect(ids()).toEqual(['sqli']);
    fireEvent.click(screen.getByRole('button', { name: 'expand all' }));
    expect(screen.getByText('src/api/a.rs')).toBeInTheDocument();
    expect(screen.queryByText('src/db/b.rs')).not.toBeInTheDocument();
    expect(screen.getByTestId('query-count')).toHaveTextContent('1 of 2');
  });

  it('a negated file: term drops files, and a row keeps the gap files left', async () => {
    vi.spyOn(api, 'getCoverageAudit').mockResolvedValue(REPORT);
    renderAt(<CoverageGapTab target="t" />);
    await screen.findByText('sqli');
    commitQuery('Filter gaps', '-file:db');
    expect(ids()).toEqual(['sqli', 'xss']);
    fireEvent.click(screen.getByRole('button', { name: 'expand all' }));
    expect(screen.getByText('src/api/a.rs')).toBeInTheDocument();
    expect(screen.queryByText('src/db/b.rs')).not.toBeInTheDocument();
    expect(screen.getByText('src/api/c.rs')).toBeInTheDocument();
  });

  it('file: terms AND per file: a row whose files each meet only one of them is gone', async () => {
    vi.spyOn(api, 'getCoverageAudit').mockResolvedValue(REPORT);
    renderAt(<CoverageGapTab target="t" />);
    await screen.findByText('sqli');
    commitQuery('Filter gaps', 'file:src file:db');
    expect(ids()).toEqual(['sqli']);
    commitQuery('Filter gaps', 'file:api file:db');
    expect(screen.getByText('No gaps match the filter.')).toBeInTheDocument();
  });

  it('says so when nothing matches', async () => {
    vi.spyOn(api, 'getCoverageAudit').mockResolvedValue(REPORT);
    renderAt(<CoverageGapTab target="t" />);
    await screen.findByText('sqli');
    commitQuery('Filter gaps', 'concern:nope');
    expect(screen.getByText('No gaps match the filter.')).toBeInTheDocument();
  });
});

describe('Audit tab', () => {
  it('filters per-concern rows by has: and an ordered severity', async () => {
    vi.spyOn(api, 'getCoverageAudit').mockResolvedValue(REPORT);
    renderAt(<CoverageAuditTab target="t" />);
    await screen.findByText('authz');
    expect(ids()).toEqual(['authz', 'sqli', 'xss']);
    commitQuery('Filter concerns', 'has:gap -has:findings');
    expect(ids()).toEqual(['xss']);
    clearQuery();
    commitQuery('Filter concerns', 'severity>=high');
    expect(ids()).toEqual(['authz', 'sqli']);
  });
});

describe('Catalog tab', () => {
  it('filters concerns by their template source and tag', async () => {
    vi.spyOn(api, 'getCoverageCatalog').mockResolvedValue(CATALOG);
    renderAt(<CoverageCatalogTab target="t" />);
    await screen.findByText('sqli');
    commitQuery('Filter concerns', 'source:inline');
    expect(ids()).toEqual(['xss']);
    clearQuery();
    commitQuery('Filter concerns', 'tag:owasp-a03');
    expect(ids()).toEqual(['sqli']);
  });
});
