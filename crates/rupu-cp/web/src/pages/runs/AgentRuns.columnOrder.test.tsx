// @vitest-environment jsdom
// AgentRuns — canonical run-table column order (table-standardization
// follow-up, I1): Status leads, then Subject (Agent) → ID (Run) → Source
// → Host → usage → Turns → Duration → Started → row actions (trailing,
// unlabeled — runs-section row-actions plan Task 3). Pure reorder — see
// AgentRuns.test.tsx for behavioral coverage and AgentRuns.actions.test.tsx
// for the row-actions themselves.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { api } from '../../lib/api';
import type { AgentRunRow, HostView } from '../../lib/api';
import AgentRuns from './AgentRuns';

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

const LOCAL_HOST: HostView = {
  id: 'local',
  name: 'Local',
  transport_kind: 'local',
  status: 'online',
  active_run_count: 0,
};

const ROW: AgentRunRow = {
  run_id: 'run-abc123',
  source: 'standalone',
  agent: 'fix-bug',
  status: 'completed',
  started_at: '2026-06-01T00:00:00Z',
  turns: 3,
  usage: {
    input_tokens: 100,
    output_tokens: 50,
    cached_tokens: 0,
    total_tokens: 150,
    cost_usd: null,
    priced: false,
    runs: 1,
  },
  host_id: 'local',
};

describe('AgentRuns — canonical column order', () => {
  it('renders headers as Status, Agent, Run, Source, Host, In, Out, Cached, Cost, Turns, Duration, Started', async () => {
    vi.spyOn(api, 'getHosts').mockResolvedValue([LOCAL_HOST]);
    vi.spyOn(api, 'getAgentRuns').mockResolvedValue([ROW]);

    const { container } = render(
      <MemoryRouter>
        <AgentRuns />
      </MemoryRouter>,
    );

    await waitFor(() => expect(screen.getByText('fix-bug')).toBeInTheDocument());

    const headers = Array.from(container.querySelectorAll('thead th')).map(
      (th) => th.textContent?.trim() ?? '',
    );
    expect(headers).toEqual([
      'Status',
      'Agent',
      'Run',
      'Source',
      'Host',
      'In',
      'Out',
      'Cached',
      'Cost',
      'Turns',
      'Duration',
      'Started',
      '', // row-actions column (unlabeled) — Task 3
    ]);
  });
});
