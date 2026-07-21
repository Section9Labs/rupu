// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect } from 'vitest';
import { render, screen, cleanup, fireEvent } from '@testing-library/react';
import { ThemeProvider } from '../theme/ThemeProvider';
import InlineFindingCard from './InlineFindingCard';
import type { FindingRecord } from '../../lib/api';

afterEach(cleanup);

const FINDING = {
  id: 'f1',
  file_path: 'src/billing.rs',
  line_range: [17, 17],
  summary: 'Missing tenant check on billing read',
  severity: 'high',
  evidence: {
    code_excerpt: 'let bill = db.get(org_id);',
    rationale: 'Line 17 checks orgId but **never** userId.',
    references: ['CWE-639'],
  },
} as unknown as FindingRecord;

function view(ui: React.ReactNode) {
  return render(<ThemeProvider>{ui}</ThemeProvider>);
}

describe('InlineFindingCard', () => {
  it('shows the collapsed summary and expands on click', () => {
    view(<InlineFindingCard finding={FINDING} stale={false} />);
    expect(screen.getByText('Missing tenant check on billing read')).toBeInTheDocument();
    // rationale hidden until expanded
    expect(screen.queryByText(/never/)).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: /Missing tenant check/ }));
    expect(screen.getByText(/never/)).toBeInTheDocument();
  });

  it('renders the stale note when stale', () => {
    view(<InlineFindingCard finding={FINDING} stale={true} />);
    fireEvent.click(screen.getByRole('button', { name: /Missing tenant check/ }));
    expect(screen.getByText(/code may have changed/i)).toBeInTheDocument();
  });
});
