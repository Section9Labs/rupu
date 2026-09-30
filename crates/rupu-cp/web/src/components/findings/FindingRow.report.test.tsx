// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { render, screen, cleanup } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import FindingRow from './FindingRow';
import type { FindingRecord } from '../../lib/api';

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

const BASE: FindingRecord = {
  id: 'f/1',
  scope: null,
  summary: 'row summary',
  severity: 'high',
  evidence: { rationale: '' },
  declared_by: null,
  declared_at: '2026-06-01T00:00:00Z',
};

function renderRow(f: FindingRecord) {
  return render(
    <MemoryRouter>
      <FindingRow finding={f} />
    </MemoryRouter>,
  );
}

describe('FindingRow — Open report link', () => {
  it('renders for a full-profile finding', () => {
    renderRow({ ...BASE, profile: 'full' });
    expect(screen.getByRole('link', { name: /Open report/ })).toHaveAttribute('href', '/findings/f%2F1');
  });

  it('is absent for summary / unprofiled findings', () => {
    renderRow({ ...BASE, profile: 'summary' });
    expect(screen.queryByRole('link', { name: /Open report/ })).not.toBeInTheDocument();
    cleanup();
    renderRow(BASE);
    expect(screen.queryByRole('link', { name: /Open report/ })).not.toBeInTheDocument();
  });
});
