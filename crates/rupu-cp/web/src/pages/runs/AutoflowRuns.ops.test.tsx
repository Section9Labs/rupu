// @vitest-environment jsdom
// AutoflowRuns — the UI-5 header actions (Doctor, Repair claims) and the
// Wakes strip, which shows only when there are wakes.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, fireEvent } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { api, type HostView } from '../../lib/api';
import { admin } from '../../lib/api3b/admin';
import AutoflowRuns from './AutoflowRuns';
import { withCustomerScope } from '../../lib/customerScopeTestUtils';

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

const LOCAL_HOST: HostView = { id: 'local', name: 'Local', transport_kind: 'local', status: 'online', active_run_count: 0 };

function renderPage(wakes: number) {
  vi.spyOn(api, 'getRegisteredHosts').mockResolvedValue([LOCAL_HOST]);
  vi.spyOn(api, 'getAutoflowRunList').mockResolvedValue([]);
  vi.spyOn(api, 'getAutoflowProblems').mockResolvedValue([]);
  vi.spyOn(api, 'getAutoflowClaims').mockResolvedValue([]);
  vi.spyOn(admin, 'getAutoflowDoctor').mockResolvedValue({ kind: 'autoflow_doctor', version: 1, ok: true, rows: [] });
  vi.spyOn(admin, 'getAutoflowWakes').mockResolvedValue({
    kind: 'autoflow_wakes',
    version: 1,
    rows: Array.from({ length: wakes }, (_, i) => ({
      wake_id: `wk_${i}`,
      state: 'queued',
      source: 'manual',
      event: 'requeue',
      entity: `github:acme/widgets/issues/${i}`,
      not_before: '2026-10-20T10:00:00Z',
      repo: 'github:acme/widgets',
    })),
  });
  render(
    <MemoryRouter initialEntries={['/activity?tab=autoflows']}>
      {withCustomerScope(<AutoflowRuns />)}
    </MemoryRouter>,
  );
}

describe('AutoflowRuns — Doctor, Repair claims, Wakes', () => {
  it('Doctor and Repair claims open their drawers', async () => {
    renderPage(0);

    fireEvent.click(screen.getByRole('button', { name: 'Doctor' }));
    expect(await screen.findByText('No problems found.')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Close' }));

    fireEvent.click(screen.getByRole('button', { name: 'Repair claims' }));
    expect(await screen.findByText('Dry run: no claim needs repair')).toBeInTheDocument();
  });

  it('shows the Wakes strip only when there are wakes', async () => {
    renderPage(2);
    expect(await screen.findByRole('region', { name: 'Autoflow wakes' })).toHaveTextContent('2 queued');
  });
});
