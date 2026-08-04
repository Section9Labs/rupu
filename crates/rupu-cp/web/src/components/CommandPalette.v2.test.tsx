// @vitest-environment jsdom
// CommandPalette v2 additions: the external `openCommandPalette()` open
// event, and the `shell="v2"` nav-page swap + moved-list-route rewrite.
// Mocks `../lib/api` and `useNavigate` the same way the rest of the CP web
// test suite does (see LauncherSheet.test.tsx) — no existing CommandPalette
// test file exists yet to imitate directly.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, fireEvent, waitFor } from '@testing-library/react';
import { api } from '../lib/api';
import type { FindingOut } from '../lib/api';

// Stub useNavigate — keep the rest of react-router-dom intact.
const navigateMock = vi.fn();
vi.mock('react-router-dom', async () => {
  const actual = await vi.importActual<typeof import('react-router-dom')>('react-router-dom');
  return { ...actual, useNavigate: () => navigateMock };
});

import CommandPalette, { openCommandPalette } from './CommandPalette';

function mockEmptyApi() {
  vi.spyOn(api, 'getRuns').mockResolvedValue([]);
  vi.spyOn(api, 'getAgents').mockResolvedValue([]);
  vi.spyOn(api, 'getWorkflows').mockResolvedValue([]);
  vi.spyOn(api, 'getAutoflowDefs').mockResolvedValue([]);
  vi.spyOn(api, 'getSessions').mockResolvedValue([]);
  vi.spyOn(api, 'getProjects').mockResolvedValue([]);
  vi.spyOn(api, 'getCoverage').mockResolvedValue([]);
  vi.spyOn(api, 'getFindings').mockResolvedValue({
    findings: [],
    summary: { total: 0, by_severity: {} } as never,
  });
  vi.spyOn(api, 'getAutoflowClaims').mockResolvedValue([]);
  vi.spyOn(api, 'getWorkers').mockResolvedValue([]);
}

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  navigateMock.mockReset();
});

describe('CommandPalette v2', () => {
  it('openCommandPalette() opens the dialog', async () => {
    mockEmptyApi();
    render(<CommandPalette />);

    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();

    openCommandPalette();

    expect(await screen.findByRole('dialog')).toBeInTheDocument();
    expect(screen.getByPlaceholderText('Search runs, agents, workflows, sessions…')).toBeInTheDocument();
  });

  it('shell="v2" surfaces Overview and not Dashboard in page results', async () => {
    mockEmptyApi();
    render(<CommandPalette shell="v2" />);

    openCommandPalette();
    const input = await screen.findByPlaceholderText('Search runs, agents, workflows, sessions…');

    // Query result titles are re-highlighted char-by-char (each matched
    // letter gets its own <mark>), so a plain getByText won't find the
    // concatenated string — match on the option's accessible name instead,
    // which aggregates the whole subtree's text.
    fireEvent.change(input, { target: { value: 'overview' } });
    await waitFor(() =>
      expect(screen.getByRole('option', { name: /Overview/i })).toBeInTheDocument(),
    );

    fireEvent.change(input, { target: { value: 'dashboard' } });
    await waitFor(() =>
      expect(screen.queryByRole('option', { name: /Dashboard/i })).not.toBeInTheDocument(),
    );
  });

  it('shell="v2" navigates a finding entity result to /security', async () => {
    mockEmptyApi();
    const finding: FindingOut = {
      id: 'f-1',
      ws_id: 'ws-1',
      project: 'demo',
      target_id: 't-1',
      file_path: 'src/main.rs',
      line_range: null,
      scope: null,
      summary: 'SQL injection in query builder',
      severity: 'high',
      concern_id: null,
      evidence: {} as never,
      declared_by: null,
      declared_at: '2026-08-01T00:00:00Z',
    };
    vi.spyOn(api, 'getFindings').mockResolvedValue({
      findings: [finding],
      summary: { total: 1, by_severity: {} } as never,
    });

    render(<CommandPalette shell="v2" />);
    openCommandPalette();
    const input = await screen.findByPlaceholderText('Search runs, agents, workflows, sessions…');

    fireEvent.change(input, { target: { value: 'SQL injection' } });
    // The title is highlighted char-by-char (each matched letter in its own
    // <mark>), so match on the row's full textContent rather than a plain
    // getByText, which only looks at each node's own direct text children.
    const titleNode = await screen.findByText(
      (_, node) => node?.textContent === 'SQL injection in query builder',
    );
    const result = titleNode.closest('[role="option"]');
    expect(result).toBeTruthy();
    fireEvent.click(result as HTMLElement);

    await waitFor(() => expect(navigateMock).toHaveBeenCalledWith('/security'));
  });
});
