// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { render, screen, cleanup } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import type { FindingRecord } from '../../lib/api';
import TriageCard from './TriageCard';

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

const FULL: FindingRecord = {
  id: 'f-full/1',
  scope: null,
  summary: 'IDOR on note fetch',
  severity: 'high',
  evidence: { rationale: '' },
  declared_by: null,
  declared_at: '2026-07-01T00:00:00Z',
  profile: 'full',
  report_summary: {
    owner: 'Unknown',
    product: 'Notebin',
    cwe: ['CWE-639'],
    root_cause: 'The **handler** trusts the client-supplied note id.',
    chain: ['a', 'b', 'sink'],
    completeness: { filled: 9, total: 11, gaps: ['owner', 'cvss_v3'] },
    has_poc: true,
    verification_status: 'confirmed',
  },
};

function renderCard(f: FindingRecord) {
  return render(
    <MemoryRouter>
      <TriageCard finding={f} />
    </MemoryRouter>,
  );
}

describe('TriageCard', () => {
  it('renders the root cause as markdown', () => {
    renderCard(FULL);
    expect(screen.getByText('handler').tagName).toBe('STRONG');
    expect(screen.getByText(/trusts the client-supplied note id/)).toBeInTheDocument();
  });

  it('joins the chain with arrows', () => {
    renderCard(FULL);
    expect(screen.getByText('a → b → sink')).toBeInTheDocument();
  });

  it('flags an Unknown owner with the gap style and leaves a known product plain', () => {
    renderCard(FULL);
    const owner = screen.getByText('Owner: Unknown');
    expect(owner).toHaveAttribute('data-gap', 'true');
    expect(owner.className).toMatch(/text-warn/);
    expect(screen.getByText('Notebin').className).not.toMatch(/text-warn/);
  });

  it('lists the gap names, PoC and verification status', () => {
    renderCard(FULL);
    expect(screen.getByText(/Report 9\/11/)).toBeInTheDocument();
    expect(screen.getByText(/unknown: owner, cvss_v3/)).toBeInTheDocument();
    expect(screen.getByText(/PoC attached/)).toBeInTheDocument();
    expect(screen.getByText(/verification: confirmed/)).toBeInTheDocument();
  });

  it('links to the full report page', () => {
    renderCard(FULL);
    expect(screen.getByRole('link', { name: /Open full report/ })).toHaveAttribute(
      'href',
      '/findings/f-full%2F1',
    );
  });

  it('renders nothing without a report_summary', () => {
    const { container } = renderCard({ ...FULL, report_summary: null });
    expect(container).toBeEmptyDOMElement();
  });
});
