// @vitest-environment jsdom
// UI-5 autoflow operations: Run now, Explain, Doctor, Repair claims, Wakes.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, fireEvent, waitFor, within } from '@testing-library/react';
import { api, ApiError, type AutoflowClaim } from '../../lib/api';
import { admin, type AutoflowDoctorReport } from '../../lib/api3b/admin';
import { AutoflowRunNow } from './AutoflowRunNow';
import { AutoflowExplainDrawer } from './AutoflowExplainDrawer';
import { AutoflowDoctorDrawer } from './AutoflowDoctorDrawer';
import { RepairClaimsDrawer } from './RepairClaimsDrawer';
import { WakesStrip } from './WakesStrip';

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

const REF = 'github:acme/widgets/issues/7';

function claim(issue_ref: string, extra: Partial<AutoflowClaim> = {}): AutoflowClaim {
  return { issue_ref, repo_ref: 'github:acme/widgets', workflow: 'issue-triage', status: 'running', updated_at: '2026-10-17T00:00:00Z', ...extra };
}

function doctor(rows: AutoflowDoctorReport['rows']): AutoflowDoctorReport {
  return { kind: 'autoflow_doctor', version: 1, ok: rows.length === 0, rows };
}

describe('AutoflowRunNow', () => {
  it('posts the ref and shows the started state with the pid', async () => {
    const run = vi.spyOn(admin, 'runAutoflow').mockResolvedValue({ started: true, name: 'issue-triage', issue: REF, pid: 4242, log: '/l' });
    render(<AutoflowRunNow name="issue-triage" onExplain={() => {}} onClose={() => {}} />);

    fireEvent.change(screen.getByLabelText('Run issue-triage for'), { target: { value: REF } });
    fireEvent.click(screen.getByRole('button', { name: 'Run' }));

    expect(run).toHaveBeenCalledWith('issue-triage', { issue: REF });
    expect(await screen.findByText(/started · pid 4242/)).toBeInTheDocument();
  });

  it("shows the API's 400 for a bare issue number inline", async () => {
    vi.spyOn(admin, 'runAutoflow').mockRejectedValue(
      new ApiError(400, 'bad', JSON.stringify({ error: 'issue `42` must be a full issue ref' })),
    );
    render(<AutoflowRunNow name="issue-triage" onExplain={() => {}} onClose={() => {}} />);

    fireEvent.change(screen.getByLabelText('Run issue-triage for'), { target: { value: '42' } });
    fireEvent.click(screen.getByRole('button', { name: 'Run' }));

    expect(await screen.findByRole('alert')).toHaveTextContent('issue `42` must be a full issue ref');
  });
});

describe('AutoflowExplainDrawer', () => {
  it('explains the issue it is given and lists its claim fields', async () => {
    vi.spyOn(admin, 'getAutoflowExplain').mockResolvedValue({
      kind: 'autoflow_explain',
      version: 1,
      item: {
        issue: REF,
        repo: 'github:acme/widgets',
        status: 'claimed',
        candidate_workflows: [],
        claim: { workflow: 'issue-triage', status: 'running', next_action: 'wait for the run', last_run: { run_id: 'run_01', status: 'running' } },
        queued_wakes: [],
        recent_cycle_events: [],
      },
    });
    render(<AutoflowExplainDrawer autoflow="issue-triage" initialRef={REF} onClose={() => {}} />);

    const dialog = screen.getByRole('dialog', { name: 'Explain issue-triage' });
    expect(await within(dialog).findByText('wait for the run')).toBeInTheDocument();
    expect(within(dialog).getByText('run_01')).toBeInTheDocument();
  });
});

describe('AutoflowDoctorDrawer', () => {
  it('lists every problem, and says so when there is none', async () => {
    vi.spyOn(admin, 'getAutoflowDoctor').mockResolvedValueOnce(
      doctor([{ scope: REF, problem: 'stale_lock', detail: 'active lock owned by `w1` remains' }]),
    );
    render(<AutoflowDoctorDrawer onClose={() => {}} />);
    expect(await screen.findByText('stale lock')).toBeInTheDocument();
    expect(screen.getByText('active lock owned by `w1` remains')).toBeInTheDocument();

    cleanup();
    vi.spyOn(admin, 'getAutoflowDoctor').mockResolvedValueOnce(doctor([]));
    render(<AutoflowDoctorDrawer onClose={() => {}} />);
    expect(await screen.findByText(/No problems found/)).toBeInTheDocument();
  });
});

describe('RepairClaimsDrawer', () => {
  it('dry-runs the claims doctor flags, then releases them and confirms from the next read', async () => {
    const other = 'github:acme/widgets/issues/9';
    vi.spyOn(admin, 'getAutoflowDoctor')
      .mockResolvedValueOnce(doctor([{ scope: REF, problem: 'stale_lock', detail: 'lock remains' }]))
      .mockResolvedValue(doctor([]));
    vi.spyOn(api, 'getAutoflowClaims')
      .mockResolvedValueOnce([claim(REF), claim(other)])
      .mockResolvedValue([claim(other)]);
    const repair = vi.spyOn(admin, 'repairClaim').mockImplementation(async (b) => ({
      issue_ref: b.issue_ref,
      dry_run: b.dry_run,
      repairs: [b.dry_run ? 'would release claim' : 'released claim'],
    }));
    const onChanged = vi.fn();
    render(<RepairClaimsDrawer onClose={() => {}} onChanged={onChanged} />);

    expect(await screen.findByText('Dry run: 1 claim would be released')).toBeInTheDocument();
    expect(repair).toHaveBeenCalledWith({ issue_ref: REF, release: true, dry_run: true });
    expect(screen.getByText('would release claim')).toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: 'Release 1 claim' }));

    await waitFor(() => expect(repair).toHaveBeenCalledWith({ issue_ref: REF, release: true, dry_run: false }));
    expect(await screen.findByText('Released 1 claim')).toBeInTheDocument();
    expect(onChanged).toHaveBeenCalled();
  });

  it('offers nothing to release when no claim has a problem', async () => {
    vi.spyOn(admin, 'getAutoflowDoctor').mockResolvedValue(doctor([]));
    vi.spyOn(api, 'getAutoflowClaims').mockResolvedValue([claim(REF)]);
    render(<RepairClaimsDrawer onClose={() => {}} onChanged={() => {}} />);

    expect(await screen.findByText('Dry run: no claim needs repair')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /^Release/ })).not.toBeInTheDocument();
  });
});

describe('WakesStrip', () => {
  it('renders nothing when there are no wakes', async () => {
    const wakes = vi.spyOn(admin, 'getAutoflowWakes').mockResolvedValue({ kind: 'autoflow_wakes', version: 1, rows: [] });
    const { container } = render(<WakesStrip />);
    await waitFor(() => expect(wakes).toHaveBeenCalled());
    expect(container).toBeEmptyDOMElement();
  });

  it('shows the queued wakes', async () => {
    vi.spyOn(admin, 'getAutoflowWakes').mockResolvedValue({
      kind: 'autoflow_wakes',
      version: 1,
      rows: [
        { wake_id: 'wk_1', state: 'queued', source: 'manual', event: 'requeue', entity: REF, not_before: '2026-10-20T10:00:00Z', repo: 'github:acme/widgets' },
        { wake_id: 'wk_0', state: 'processed', source: 'poll', event: 'issue.opened', entity: REF, not_before: '2026-10-19T10:00:00Z', repo: 'github:acme/widgets' },
      ],
    });
    render(<WakesStrip />);

    expect(await screen.findByText('1 queued')).toBeInTheDocument();
    expect(screen.getByText('acme/widgets#7')).toBeInTheDocument();
    expect(screen.getByText(/1 processed recently/)).toBeInTheDocument();
  });
});
