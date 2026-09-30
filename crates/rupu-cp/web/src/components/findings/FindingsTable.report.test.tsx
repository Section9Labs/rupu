// @vitest-environment jsdom
// FindingsTable — Report column + triage-card row expansion for full-profile
// findings; summary rows keep the evidence panel.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { render, screen, cleanup, fireEvent, within } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { FindingsTable } from './FindingsTable';
import type { FindingOut, FindingRecord } from '../../lib/api';

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

function base(overrides: Partial<FindingRecord>): FindingRecord {
  return {
    id: 'f1',
    scope: null,
    summary: 's',
    severity: 'high',
    evidence: { rationale: 'plain rationale text' },
    declared_by: null,
    declared_at: '2026-06-01T00:00:00Z',
    ...overrides,
  };
}

const FULL = base({
  id: 'full-1',
  summary: 'Full report finding',
  profile: 'full',
  report_summary: {
    owner: 'Unknown',
    product: 'Notebin',
    cwe: ['CWE-639'],
    root_cause: 'Trusts the client-supplied id.',
    chain: ['a', 'sink'],
    completeness: { filled: 9, total: 11, gaps: ['owner', 'cvss_v3'] },
    has_poc: true,
    verification_status: null,
  },
});

const SUMMARY = base({ id: 'sum-1', summary: 'Summary only finding', profile: 'summary' });

function renderTable(rows: FindingRecord[]) {
  return render(
    <MemoryRouter>
      <FindingsTable findings={rows} />
    </MemoryRouter>,
  );
}

function rowOf(text: string): HTMLElement {
  return screen.getByText(text).closest('tr') as HTMLElement;
}

describe('FindingsTable — Report column', () => {
  it('shows completeness and PoC for a full row, "summary" for a summary row', () => {
    renderTable([FULL, SUMMARY]);
    expect(screen.getByRole('columnheader', { name: /Report/ })).toBeInTheDocument();
    const full = within(rowOf('Full report finding'));
    expect(full.getByText(/9\/11/)).toBeInTheDocument();
    expect(full.getByText('PoC')).toBeInTheDocument();
    expect(within(rowOf('Summary only finding')).getByText('summary')).toBeInTheDocument();
  });

  it('prefers report_summary.cwe over concern-derived CWE in the CWE column', () => {
    renderTable([{ ...FULL, concern_id: 'cwe-top25:cwe-787-oob' }]);
    const link = within(rowOf('Full report finding')).getByRole('link', { name: 'CWE-639' });
    expect(link).toHaveAttribute('href', 'https://cwe.mitre.org/data/definitions/639.html');
  });

  it('falls back to cweFromFinding for a summary row', () => {
    renderTable([{ ...SUMMARY, concern_id: 'cwe-top25:cwe-787-oob' }]);
    expect(
      within(rowOf('Summary only finding')).getByRole('link', { name: 'CWE-787' }),
    ).toBeInTheDocument();
  });
});

describe('FindingsTable — expanded detail', () => {
  it('expands a full-profile row to the triage card', () => {
    renderTable([FULL]);
    fireEvent.click(within(rowOf('Full report finding')).getByRole('button', { name: 'Expand row' }));
    expect(screen.getByText('Trusts the client-supplied id.')).toBeInTheDocument();
    expect(screen.getByRole('link', { name: /Open full report/ })).toHaveAttribute(
      'href',
      '/findings/full-1',
    );
  });

  it('expands a summary row to the evidence panel, not the triage card', () => {
    renderTable([SUMMARY]);
    fireEvent.click(within(rowOf('Summary only finding')).getByRole('button', { name: 'Expand row' }));
    expect(screen.getByText('plain rationale text')).toBeInTheDocument();
    expect(screen.queryByRole('link', { name: /Open full report/ })).not.toBeInTheDocument();
  });
});

describe('FindingsTable — code href', () => {
  it('still deep-links the location via codeHref', () => {
    const f: FindingOut = {
      ...base({ file_path: 'src/a b.rs', line_range: [3, 4] }),
      ws_id: 'ws 1',
      project: 'p',
      target_id: 't',
    };
    renderTable([f]);
    expect(screen.getByRole('button', { name: /src\/a b\.rs/ })).toBeInTheDocument();
  });
});
