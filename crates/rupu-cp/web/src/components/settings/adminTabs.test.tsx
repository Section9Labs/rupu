// @vitest-environment jsdom
// UI-5 Settings tabs: Automation (cron + events, dry run / tick now),
// Maintenance (dry-run counts, confirmed prune), Accounts (state chips, the
// login command, binding rules + Add rule).

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, fireEvent, waitFor, within } from '@testing-library/react';
import { ApiError } from '../../lib/api';
import { admin, type AccountAuth, type PruneRequest, type ScmAccountRow } from '../../lib/api3b/admin';
import { AutomationTab } from './AutomationTab';
import { MaintenanceTab } from './MaintenanceTab';
import { AccountsTab } from './AccountsTab';

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

const err = (status: number, message: string) => new ApiError(status, message, JSON.stringify({ error: message }));

describe('AutomationTab', () => {
  function stubLists() {
    vi.spyOn(admin, 'getCronList').mockResolvedValue({
      kind: 'cron_list',
      version: 1,
      rows: [{ name: 'nightly-audit', schedule: '0 3 * * *', next_utc: '2026-10-21 03:00:00', in_seconds: 3 * 3600 + 60 }],
    });
    vi.spyOn(admin, 'getCronEvents').mockResolvedValue({
      kind: 'cron_events',
      version: 1,
      rows: [{ name: 'issue-triage', event: 'issue.opened', sources: ['github:acme/widgets'], cursor: '2026-10-20T09:00:00Z' }],
      summary: { poll_sources: ['github:acme/widgets'] },
    });
  }

  it('lists the cron schedules and the event triggers, with the loop cadence', async () => {
    stubLists();
    render(<AutomationTab tick={{ enabled: true, intervalSecs: 60 }} />);

    expect(await screen.findByText('nightly-audit')).toBeInTheDocument();
    expect(screen.getByText('0 3 * * *')).toBeInTheDocument();
    expect(screen.getByText('in 3h')).toBeInTheDocument();
    expect(screen.getByText('issue.opened')).toBeInTheDocument();
    expect(screen.getByText('The cp loop ticks every 60 s.')).toBeInTheDocument();
  });

  it('Dry run lists what would fire and changes nothing', async () => {
    stubLists();
    const tick = vi.spyOn(admin, 'cronTick').mockResolvedValue({
      kind: 'cron_tick',
      dry_run: true,
      would_fire: [{ tier: 'cron', workflow: 'nightly-audit' }],
    });
    render(<AutomationTab tick={{ enabled: true, intervalSecs: 60 }} />);
    await screen.findByText('nightly-audit');

    fireEvent.click(screen.getByRole('button', { name: 'Dry run' }));

    expect(tick).toHaveBeenCalledWith({ dry_run: true });
    const box = await screen.findByRole('status', { name: 'Dry run result' });
    expect(box).toHaveTextContent('Would fire 1 workflow');
    expect(within(box).getByText('nightly-audit')).toBeInTheDocument();
  });

  it('Tick now shows the pending state, and the 409 when a tick is already running', async () => {
    stubLists();
    const tick = vi
      .spyOn(admin, 'cronTick')
      .mockResolvedValueOnce({ started: true, dry_run: false })
      .mockRejectedValueOnce(err(409, 'a cron tick is already running'));
    render(<AutomationTab tick={{ enabled: true, intervalSecs: 60 }} />);
    await screen.findByText('nightly-audit');

    fireEvent.click(screen.getByRole('button', { name: 'Tick now' }));
    expect(await screen.findByText('tick started')).toBeInTheDocument();
    expect(tick).toHaveBeenCalledWith({ dry_run: false });

    fireEvent.click(screen.getByRole('button', { name: 'Tick now' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('a cron tick is already running');
  });
});

describe('MaintenanceTab', () => {
  it('shows each kind’s dry-run count, then prunes after the confirm', async () => {
    let pruned = false;
    const prune = vi.spyOn(admin, 'prune').mockImplementation(async (b: PruneRequest) => {
      const rows = b.kind === 'transcripts' && !pruned ? [{ run_id: 'a' }, { run_id: 'b' }, { run_id: 'c' }] : [];
      if (!b.dry_run) {
        pruned = true;
        return { kind: b.kind, dry_run: false, removed: rows, errors: [] };
      }
      return { kind: b.kind, dry_run: true, would_remove: rows, errors: [] };
    });
    render(<MaintenanceTab retention={{ transcripts: '30d', sessions: '30d' }} />);

    const row = await screen.findByRole('row', { name: /Archived transcripts/ });
    await waitFor(() => expect(within(row).getByText('3 transcripts')).toBeInTheDocument());
    expect(prune).toHaveBeenCalledWith({ kind: 'transcripts', older_than: '30d', dry_run: true });

    fireEvent.click(within(row).getByRole('button', { name: 'Prune 3' }));
    expect(screen.getByText("Delete 3 archived transcripts? This can't be undone.")).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Delete' }));

    await waitFor(() => expect(prune).toHaveBeenCalledWith({ kind: 'transcripts', older_than: '30d', dry_run: false }));
    expect(await screen.findByText('Removed 3 transcripts')).toBeInTheDocument();
    await waitFor(() => expect(within(row).getByText('nothing')).toBeInTheDocument());
  });

  it('re-counts when the older-than field changes', async () => {
    const prune = vi.spyOn(admin, 'prune').mockImplementation(async (b) => ({ kind: b.kind, dry_run: true, would_remove: [], errors: [] }));
    render(<MaintenanceTab retention={{}} />);
    const row = await screen.findByRole('row', { name: /Netflow ledgers/ });

    fireEvent.change(within(row).getByLabelText('Netflow ledgers older than'), { target: { value: '7d' } });

    await waitFor(() => expect(prune).toHaveBeenCalledWith({ kind: 'netflow', older_than: '7d', dry_run: true }));
  });

  it('shows a bad older-than and a 501 on the row they belong to', async () => {
    vi.spyOn(admin, 'prune').mockImplementation(async (b) => {
      if (b.kind === 'sessions') throw err(501, 'pruning sessions needs rupu cp serve');
      if (b.kind === 'netflow') throw err(400, 'invalid duration `soon`');
      return { kind: b.kind, dry_run: true, would_remove: [], errors: [] };
    });
    render(<MaintenanceTab retention={{}} />);

    const sessions = await screen.findByRole('row', { name: /Archived sessions/ });
    await waitFor(() => expect(sessions).toHaveTextContent('pruning sessions needs rupu cp serve'));
    const netflow = screen.getByRole('row', { name: /Netflow ledgers/ });
    expect(netflow).toHaveTextContent('invalid duration `soon`');
  });
});

describe('AccountsTab', () => {
  const AUTH: AccountAuth[] = [
    { account: 'anthropic', kind: 'anthropic', api_key: false, sso: { expires_at: '2026-10-19T00:00:00Z', expired: true, refreshable: true, summary: 'expired — refreshes on use' }, state: 'ok' },
    { account: 'openai', kind: 'openai', api_key: true, sso: null, state: 'ok' },
    { account: 'gl.work', kind: 'gitlab', api_key: false, sso: { expires_at: '2026-10-01T00:00:00Z', expired: true, refreshable: false, summary: 'expired — re-login' }, state: 'needs_login' },
    { account: 'jira', kind: 'jira', api_key: false, sso: null, state: 'absent' },
  ];
  const SCM: ScmAccountRow[] = [
    { account: 'gl.work', kind: 'gitlab', base_url: 'https://gitlab.corp.example', rules: [{ owner: 'acme/*' }], credentials: true, state: 'needs_login' },
  ];

  function stub() {
    vi.spyOn(admin, 'getAuthStatus').mockResolvedValue(AUTH);
    vi.spyOn(admin, 'getScmAccounts').mockResolvedValue(SCM);
    return vi
      .spyOn(admin, 'getScmRules')
      .mockResolvedValueOnce([{ owner: 'acme/*', account: 'gl.work' }])
      .mockResolvedValue([
        { owner: 'acme/*', account: 'gl.work' },
        { path: '~/Code/side/*', account: 'github' },
      ]);
  }

  it('reads an account that refreshes itself as ok, and gives a needs-login row its command', async () => {
    stub();
    render(<AccountsTab />);

    const anthropic = await screen.findByRole('row', { name: /anthropic/ });
    expect(within(anthropic).getByText('ok · refreshes itself')).toBeInTheDocument();
    const gl = screen.getByRole('row', { name: /gl\.work/ });
    expect(within(gl).getByText('needs login')).toBeInTheDocument();
    expect(within(gl).getByText('rupu auth login --account gl.work --mode sso')).toBeInTheDocument();
    expect(within(gl).getByText('scm · gitlab.corp.example')).toBeInTheDocument();
    // An account with no credential is behind a toggle, not in the table.
    expect(screen.queryByRole('row', { name: /jira/ })).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: /1 account with no credential/ })).toBeInTheDocument();
  });

  it('lists the binding rules and adds one, confirming it from the next read', async () => {
    const rules = stub();
    const add = vi.spyOn(admin, 'addScmRule').mockResolvedValue({
      rule: { path: '~/Code/side/*', account: 'github' },
      rules: [],
    });
    render(<AccountsTab />);
    expect(await screen.findByText('owner acme/*')).toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: 'Add rule' }));
    fireEvent.change(screen.getByLabelText('Match on'), { target: { value: 'path' } });
    fireEvent.change(screen.getByLabelText('Pattern'), { target: { value: '~/Code/side/*' } });
    fireEvent.change(screen.getByLabelText('Account'), { target: { value: 'github' } });
    fireEvent.click(screen.getByRole('button', { name: 'Save rule' }));

    await waitFor(() => expect(add).toHaveBeenCalledWith({ path: '~/Code/side/*', account: 'github' }));
    expect(await screen.findByText('path ~/Code/side/*')).toBeInTheDocument();
    expect(rules).toHaveBeenCalledTimes(2);
  });

  it('shows the 400 of a rule the server refuses', async () => {
    stub();
    vi.spyOn(admin, 'addScmRule').mockRejectedValue(err(400, 'a rule needs exactly one of owner or path'));
    render(<AccountsTab />);
    await screen.findByText('owner acme/*');

    fireEvent.click(screen.getByRole('button', { name: 'Add rule' }));
    fireEvent.change(screen.getByLabelText('Pattern'), { target: { value: 'x' } });
    fireEvent.click(screen.getByRole('button', { name: 'Save rule' }));

    expect(await screen.findByRole('alert')).toHaveTextContent('a rule needs exactly one of owner or path');
  });
});
