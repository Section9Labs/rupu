// @vitest-environment jsdom
// New agent — clicking "New agent" on the Agents list routes to the
// dedicated full-page Agent Builder rather than opening a modal.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { render, screen, cleanup, fireEvent } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { api } from '../lib/api';

const navigateMock = vi.fn();
vi.mock('react-router-dom', async () => {
  const actual = await vi.importActual<typeof import('react-router-dom')>('react-router-dom');
  return { ...actual, useNavigate: () => navigateMock };
});

vi.mock('../components/charts/UsageBarChart', () => ({
  __esModule: true,
  default: () => <div data-testid="usage-bar-chart" />,
}));

import Agents from './Agents';

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  navigateMock.mockReset();
});

describe('New agent — Agent Builder', () => {
  it('navigates to the full-page /agents/new instead of opening a modal', async () => {
    vi.spyOn(api, 'getAgents').mockResolvedValue([]);

    render(
      <MemoryRouter>
        <Agents />
      </MemoryRouter>,
    );

    fireEvent.click(await screen.findByRole('button', { name: 'New agent' }));

    // No modal opens — the button routes to the dedicated full page.
    expect(navigateMock).toHaveBeenCalledWith('/agents/new');
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  });
});
