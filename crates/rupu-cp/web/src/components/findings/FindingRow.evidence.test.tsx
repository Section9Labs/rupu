// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect } from 'vitest';
import { render, screen, cleanup, fireEvent } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import FindingRow from './FindingRow';
import type { FindingRecord } from '../../lib/api';

afterEach(cleanup);

const FINDING = {
  id: 'f1',
  file_path: 'src/billing.rs',
  line_range: [17, 19],
  severity: 'high',
  summary: 's',
  evidence: { rationale: 'uses `parseToken`', references: [] },
} as unknown as FindingRecord;

describe('FindingRow evidence', () => {
  it('expands the evidence toggle and renders the rationale as markdown', () => {
    render(
      <MemoryRouter initialEntries={['/findings']}>
        <FindingRow finding={FINDING} wsId="ws1" />
      </MemoryRouter>,
    );
    expect(screen.queryByText('parseToken')).toBeNull();
    fireEvent.click(screen.getByRole('button', { name: /evidence/i }));
    const code = screen.getByText('parseToken');
    expect(code.tagName).toBe('CODE');
  });
});
