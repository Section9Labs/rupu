// @vitest-environment jsdom
// Project → Config → Repositories (UI-5): the tracked checkouts inside this
// project, with Prefer, a confirmed Forget, and Attach.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, fireEvent, waitFor, within } from '@testing-library/react';
import { ApiError } from '../../lib/api';
import { admin, type TrackedRepo } from '../../lib/api3b/admin';
import { ProjectRepositories } from './ProjectRepositories';

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

const WIDGETS: TrackedRepo = {
  id: 'github_acme_widgets',
  repo: 'github:acme/widgets',
  preferred_path: '/src/other-widgets',
  known_paths: ['/src/other-widgets', '/src/widgets'],
  default_branch: 'main',
  last_seen_at: '2026-10-19T08:00:00Z',
};
const ELSEWHERE: TrackedRepo = {
  id: 'github_acme_gadgets',
  repo: 'github:acme/gadgets',
  preferred_path: '/src/gadgets',
  known_paths: ['/src/gadgets'],
  last_seen_at: '2026-10-19T08:00:00Z',
};

describe('ProjectRepositories', () => {
  it('lists only the checkouts inside this project', async () => {
    vi.spyOn(admin, 'getTrackedRepos').mockResolvedValue([WIDGETS, ELSEWHERE]);
    render(<ProjectRepositories projectPath="/src/widgets" />);

    const row = await screen.findByRole('row', { name: /github:acme\/widgets/ });
    expect(within(row).getByText('project root')).toHaveAttribute('title', '/src/widgets');
    expect(screen.queryByText('github:acme/gadgets')).not.toBeInTheDocument();
  });

  it('Prefer makes the checkout preferred, confirmed by the next read', async () => {
    vi.spyOn(admin, 'getTrackedRepos')
      .mockResolvedValueOnce([WIDGETS])
      .mockResolvedValue([{ ...WIDGETS, preferred_path: '/src/widgets' }]);
    const prefer = vi.spyOn(admin, 'preferRepo').mockResolvedValue({ ...WIDGETS, preferred_path: '/src/widgets' });
    render(<ProjectRepositories projectPath="/src/widgets" />);

    fireEvent.click(await screen.findByRole('button', { name: 'Prefer /src/widgets' }));

    expect(prefer).toHaveBeenCalledWith({ repo: 'github:acme/widgets', path: '/src/widgets' });
    expect(await screen.findByText('preferred')).toBeInTheDocument();
  });

  it('Forget asks first, then forgets only that path', async () => {
    vi.spyOn(admin, 'getTrackedRepos').mockResolvedValueOnce([WIDGETS]).mockResolvedValue([]);
    const forget = vi.spyOn(admin, 'forgetRepo').mockResolvedValue({ repo: WIDGETS.repo, path: '/src/widgets', remaining: null });
    render(<ProjectRepositories projectPath="/src/widgets" />);

    fireEvent.click(await screen.findByRole('button', { name: 'Forget /src/widgets' }));
    expect(forget).not.toHaveBeenCalled();
    expect(screen.getByRole('group', { name: 'Confirm forget' })).toHaveTextContent(
      'Forget the project root as a checkout of github:acme/widgets?',
    );
    fireEvent.click(screen.getByRole('button', { name: 'Forget' }));

    await waitFor(() => expect(forget).toHaveBeenCalledWith('github_acme_widgets', '/src/widgets'));
    expect(await screen.findByText(/No tracked checkout/)).toBeInTheDocument();
  });

  it('Attach sends the repo and the path, and shows the server’s 400', async () => {
    vi.spyOn(admin, 'getTrackedRepos').mockResolvedValue([]);
    const attach = vi
      .spyOn(admin, 'attachRepo')
      .mockRejectedValue(new ApiError(400, 'x', JSON.stringify({ error: '/src/widgets is a checkout of github:acme/other' })));
    render(<ProjectRepositories projectPath="/src/widgets" />);

    fireEvent.click(await screen.findByRole('button', { name: 'Attach' }));
    const dialog = screen.getByRole('dialog', { name: 'Attach a checkout' });
    expect(within(dialog).getByLabelText('Path')).toHaveValue('/src/widgets');
    fireEvent.change(within(dialog).getByLabelText('Repository'), { target: { value: 'github:acme/widgets' } });
    fireEvent.click(within(dialog).getByRole('button', { name: 'Attach' }));

    await waitFor(() => expect(attach).toHaveBeenCalledWith({ repo: 'github:acme/widgets', path: '/src/widgets' }));
    expect(await within(dialog).findByRole('alert')).toHaveTextContent('/src/widgets is a checkout of github:acme/other');
  });
});
