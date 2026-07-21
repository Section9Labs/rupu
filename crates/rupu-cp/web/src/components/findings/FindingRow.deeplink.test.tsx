// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect } from 'vitest';
import { render, screen, cleanup, fireEvent } from '@testing-library/react';
import { MemoryRouter, useLocation } from 'react-router-dom';
import FindingRow from './FindingRow';
import type { FindingRecord } from '../../lib/api';

afterEach(cleanup);

const FINDING = {
  id: 'f1',
  file_path: 'src/billing.rs',
  line_range: [17, 19],
  severity: 'high',
  summary: 's',
  evidence: { rationale: '', references: [] },
} as unknown as FindingRecord;

function LocationProbe() {
  const loc = useLocation();
  return <div data-testid="loc">{loc.pathname + loc.search}</div>;
}

describe('FindingRow deep-link', () => {
  it('navigates to the Code tab at the finding file:line', () => {
    render(
      <MemoryRouter initialEntries={['/findings']}>
        <FindingRow finding={FINDING} wsId="ws1" />
        <LocationProbe />
      </MemoryRouter>,
    );
    fireEvent.click(screen.getByRole('button', { name: /src\/billing\.rs/ }));
    expect(screen.getByTestId('loc')).toHaveTextContent(
      '/projects/ws1/code?path=src%2Fbilling.rs&line=17',
    );
  });
});
