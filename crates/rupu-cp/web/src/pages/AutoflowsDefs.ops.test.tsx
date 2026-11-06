// @vitest-environment jsdom
// AutoflowsDefs — the UI-5 row actions: Run now opens the inline issue-ref
// field under the table, Explain opens the drawer; neither navigates.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { render, screen, cleanup, fireEvent, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { api, type AutoflowDefRow } from '../lib/api';
import { admin } from '../lib/api3b/admin';
import AutoflowsDefs from './AutoflowsDefs';
import { withCustomerScope } from '../lib/customerScopeTestUtils';

const ROW: AutoflowDefRow = { name: 'issue-triage', slug: 'issue-triage', trigger: 'event', scope: 'global', scope_kind: 'global', enabled: true };

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

function renderPage() {
  vi.spyOn(api, 'getAutoflowDefs').mockResolvedValue([ROW]);
  render(
    <MemoryRouter initialEntries={['/library?tab=autoflows']}>
      {withCustomerScope(<AutoflowsDefs />)}
    </MemoryRouter>,
  );
}

describe('AutoflowsDefs — Run now and Explain', () => {
  it('Run now opens the issue-ref field without navigating, and runs the row by name', async () => {
    const run = vi.spyOn(admin, 'runAutoflow').mockResolvedValue({ started: true, name: 'issue-triage', issue: 'github:acme/widgets/issues/3', pid: 9, log: '' });
    renderPage();
    const btn = await screen.findByRole('button', { name: 'Run issue-triage now' });

    expect(fireEvent.click(btn)).toBe(false);
    fireEvent.change(screen.getByLabelText('Run issue-triage for'), { target: { value: 'github:acme/widgets/issues/3' } });
    fireEvent.click(screen.getByRole('button', { name: 'Run' }));

    await waitFor(() => expect(run).toHaveBeenCalledWith('issue-triage', { issue: 'github:acme/widgets/issues/3' }));
  });

  it('Explain opens the drawer for the row', async () => {
    renderPage();
    fireEvent.click(await screen.findByRole('button', { name: 'Explain issue-triage' }));

    expect(screen.getByRole('dialog', { name: 'Explain issue-triage' })).toBeInTheDocument();
  });
});
