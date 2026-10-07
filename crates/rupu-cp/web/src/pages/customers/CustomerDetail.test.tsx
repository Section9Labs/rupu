// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { MemoryRouter, Route, Routes, useLocation } from 'react-router-dom';
import {
  api,
  ApiError,
  type CustomerDetail as CustomerDetailDto,
  type AgentRunRow,
  type CustomerScope,
  type FindingOut,
  type ProjectRow,
  type RunListRow,
  type UsageResponse,
} from '../../lib/api';
import { REG_LOCAL, REG_PROD, callsFor, onlyHost } from '../../lib/perHost/testUtils';
import * as scopeLib from '../../lib/customerScope';
import CustomerDetail from './CustomerDetail';

const USAGE = {
  input_tokens: 1000,
  output_tokens: 500,
  cached_tokens: 0,
  total_tokens: 1500,
  cost_usd: 0,
  priced: true,
  runs: 0,
};

const TINT = { light: '#111111', dark: '#eeeeee' };
const ACME_REF = { slug: 'acme', name: 'Acme Corp', tint: TINT, archived: false };

function proj(ws: string, cost: number, customer: ProjectRow['customer'] = ACME_REF): ProjectRow {
  return {
    ws_id: ws,
    name: `proj-${ws}`,
    path: `/src/${ws}`,
    created_at: '2026-01-01T00:00:00Z',
    usage: { ...USAGE, cost_usd: cost },
    run_count: 3,
    customer,
  };
}

function detail(over: Partial<CustomerDetailDto> = {}): CustomerDetailDto {
  return {
    customer: {
      slug: 'acme',
      name: 'Acme Corp',
      notes: 'Pen-test retainer, renews in March.',
      contact: 'ops@acme.test',
      color: null,
      tint: TINT,
      archived: false,
      created_at: '2026-09-01T12:00:00Z',
    },
    rollup: {
      projects: 2,
      run_count: 40,
      usage: { ...USAGE, total_tokens: 1_500_000, cost_usd: 30, pricing_error: 'priced at global rates' },
      findings_open: 5,
      last_active: '2026-10-05T00:00:00Z',
    },
    projects: [proj('a', 20), proj('b', 10)],
    default_account: { account: 'acme-prod', locked_by: 'customer', inherited: false },
    layer_error: null,
    config_path: '~/.rupu/customers/acme/config.toml',
    ...over,
  };
}

const RUN: RunListRow = {
  codename: 'cobalt-harbor',
  codename_derived: false,
  id: 'run_1',
  workflow_name: 'nightly-review',
  status: 'completed',
  started_at: '2026-10-05T00:00:00Z',
  trigger: 'manual',
  turns: 3,
  usage: USAGE,
  customer: 'acme',
  host_id: 'local',
};

let setScope: ReturnType<typeof vi.fn<(n: CustomerScope) => void>>;
let reload: ReturnType<typeof vi.fn<() => void>>;
let getCustomer: ReturnType<typeof vi.spyOn>;
let getRuns: ReturnType<typeof vi.spyOn>;
let getWorkflowRuns: ReturnType<typeof vi.spyOn>;
let getAgentRuns: ReturnType<typeof vi.spyOn>;
let getFindings: ReturnType<typeof vi.spyOn>;
let getUsage: ReturnType<typeof vi.spyOn>;
let getUsageRuns: ReturnType<typeof vi.spyOn>;
let getUsageOutliers: ReturnType<typeof vi.spyOn>;

const USAGE_RESPONSE: UsageResponse = {
  summary: { ...USAGE, cost_usd: 7.5, priced: true, runs: 4, total_tokens: 1500 },
  breakdown: [],
  unpriced: { models: [], rows: 0 },
  hosts: [
    { host_id: 'local', name: 'Local', transport_kind: 'local', state: 'ok', captured_at: '2026-10-05T00:00:00Z', reason: null },
  ],
};

function Where() {
  const l = useLocation();
  return <div data-testid="where">{l.pathname + l.search}</div>;
}

function mount(path = '/customers/acme') {
  return render(
    <MemoryRouter initialEntries={[path]}>
      <Routes>
        <Route path="/customers" element={<div>customers list</div>} />
        <Route path="/customers/:slug" element={<CustomerDetail />} />
        <Route path="/customers/:slug/:tab" element={<CustomerDetail />} />
      </Routes>
      <Where />
    </MemoryRouter>,
  );
}

beforeEach(() => {
  setScope = vi.fn<(n: CustomerScope) => void>();
  reload = vi.fn<() => void>();
  vi.spyOn(scopeLib, 'useCustomerScope').mockReturnValue({
    scope: 'acme',
    customer: null,
    customers: [],
    setScope,
    reload,
    rejectScope: vi.fn(),
    notice: null,
    noticeSeq: 0,
  });
  getCustomer = vi.spyOn(api, 'getCustomer').mockResolvedValue(detail());
  getRuns = vi.spyOn(api, 'getRuns').mockResolvedValue([RUN]);
  // The Runs / Findings / Usage tabs embed the CP's own tables and charts.
  vi.spyOn(api, 'getRegisteredHosts').mockResolvedValue([REG_LOCAL]);
  getWorkflowRuns = vi.spyOn(api, 'getWorkflowRuns').mockResolvedValue([RUN]);
  getAgentRuns = vi.spyOn(api, 'getAgentRuns').mockResolvedValue([]);
  getFindings = vi.spyOn(api, 'getFindings').mockResolvedValue({
    findings: [],
    summary: { total: 0, critical: 0, high: 0, medium: 0, low: 0, info: 0 },
  });
  getUsage = vi.spyOn(api, 'getUsage').mockResolvedValue(USAGE_RESPONSE);
  getUsageRuns = vi.spyOn(api, 'getUsageRuns').mockResolvedValue([]);
  getUsageOutliers = vi.spyOn(api, 'getUsageOutliers').mockResolvedValue([]);
});
afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

function tile(id: string) {
  return screen.getByTestId(`tile-${id}`);
}

describe('CustomerDetail header and tiles', () => {
  it('renders the header from getCustomer', async () => {
    mount();
    expect(await screen.findByRole('heading', { level: 1, name: 'Acme Corp' })).toBeInTheDocument();
    expect(getCustomer).toHaveBeenCalledWith('acme', '30d');
    const nav = screen.getByRole('navigation', { name: 'Breadcrumb' });
    expect(within(nav).getByRole('link', { name: 'Customers' })).toHaveAttribute('href', '/customers');
    expect(within(nav).getByText('Acme Corp')).toBeInTheDocument();
    expect(screen.getByTestId('customer-slug')).toHaveTextContent('acme');
    expect(screen.getByText('ops@acme.test')).toBeInTheDocument();
    expect(screen.getByText(/^created /)).toBeInTheDocument();
    expect(screen.getByText('~/.rupu/customers/acme/config.toml')).toBeInTheDocument();
    expect(screen.getByText('Pen-test retainer, renews in March.')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Edit details' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Archive' })).toBeInTheDocument();
  });

  it('shows the config path the server reports, not a hard-coded one', async () => {
    getCustomer.mockResolvedValue(detail({ config_path: '/srv/rupu-home/customers/acme/config.toml' }));
    mount();
    await screen.findByRole('heading', { level: 1, name: 'Acme Corp' });
    expect(screen.getByText('/srv/rupu-home/customers/acme/config.toml')).toBeInTheDocument();
    expect(screen.queryByText('~/.rupu/customers/acme/config.toml')).toBeNull();
  });

  it('names the hosts the rollup left out in the warn banner', async () => {
    vi.mocked(api.getRegisteredHosts).mockResolvedValue([REG_LOCAL, REG_PROD]);
    getCustomer.mockResolvedValue(detail({ rollup: { ...detail().rollup, hosts_without_customer: ['host_prod'] } }));
    mount();
    await screen.findByRole('heading', { level: 1, name: 'Acme Corp' });
    // Named by the registered host's name, not its id.
    await waitFor(() =>
      expect(screen.getByTestId('hosts-without-customer')).toHaveTextContent(/^prod runs an older rupu .* not counted as zero/),
    );
  });

  it('renders the five tiles from the rollup', async () => {
    mount();
    await screen.findByRole('heading', { level: 1, name: 'Acme Corp' });
    expect(tile('projects')).toHaveTextContent('2');
    expect(tile('runs')).toHaveTextContent('Runs · 30d');
    expect(tile('runs')).toHaveTextContent('40');
    expect(tile('findings')).toHaveTextContent('5');
    expect(within(tile('findings')).getByText('needs attention').closest('p')).toHaveClass('text-err');
    expect(tile('cost')).toHaveTextContent('Cost · 30d');
    expect(tile('cost')).toHaveTextContent('$30.00');
    expect(tile('cost')).toHaveTextContent('1.5M tokens');
    expect(within(tile('cost')).getByRole('img', { name: /Pricing unavailable/ })).toHaveAttribute(
      'title',
      'priced at global rates',
    );
    expect(tile('bills')).toHaveTextContent('acme-prod');
    expect(tile('bills')).toHaveTextContent('locked by customer');
  });

  it('BILLS TO says where the account comes from', async () => {
    getCustomer.mockResolvedValue(
      detail({ default_account: { account: 'work', locked_by: null, inherited: true } }),
    );
    mount();
    await waitFor(() => expect(tile('bills')).toHaveTextContent('inherits global'));
    cleanup();

    getCustomer.mockResolvedValue(
      detail({ default_account: { account: 'acme-dev', locked_by: null, inherited: false } }),
    );
    mount();
    await waitFor(() => expect(tile('bills')).toHaveTextContent('customer default'));
    expect(tile('bills')).toHaveTextContent('acme-dev');
  });

  it('range switch refetches the rollup', async () => {
    mount();
    await screen.findByRole('heading', { level: 1, name: 'Acme Corp' });
    fireEvent.click(screen.getByRole('button', { name: '7d' }));
    await waitFor(() => expect(getCustomer).toHaveBeenLastCalledWith('acme', '7d'));
    await waitFor(() => expect(tile('runs')).toHaveTextContent('Runs · 7d'));
  });

  it('shows a layer_error banner linking to the Config tab', async () => {
    getCustomer.mockResolvedValue(detail({ layer_error: 'expected `=` at line 3', default_account: null }));
    mount();
    const banner = await screen.findByRole('alert');
    expect(banner).toHaveTextContent(
      "This customer's config layer doesn't parse — runs of its projects will fail until it's fixed.",
    );
    expect(banner).toHaveTextContent('expected `=` at line 3');
    expect(within(banner).getByRole('link', { name: /Config tab/ })).toHaveAttribute(
      'href',
      '/customers/acme/config',
    );
  });

  it('shows "Customer not found" on a 404', async () => {
    getCustomer.mockRejectedValue(new ApiError(404, 'nf', JSON.stringify({ error: 'no such customer' })));
    mount('/customers/ghost');
    expect(await screen.findByText('Customer not found')).toBeInTheDocument();
  });

  it('Archive archives and refetches; an archived customer shows Unarchive and a badge', async () => {
    const archive = vi.spyOn(api, 'archiveCustomer').mockResolvedValue(detail().customer);
    mount();
    await screen.findByRole('heading', { level: 1, name: 'Acme Corp' });
    getCustomer.mockResolvedValue(detail({ customer: { ...detail().customer, archived: true } }));
    fireEvent.click(screen.getByRole('button', { name: 'Archive' }));
    await waitFor(() => expect(archive).toHaveBeenCalledWith('acme', true));
    expect(await screen.findByRole('button', { name: 'Unarchive' })).toBeInTheDocument();
    expect(screen.getByText('Archived')).toBeInTheDocument();
    expect(reload).toHaveBeenCalled();
  });
});

describe('CustomerDetail tabs', () => {
  it('an unknown tab falls back to overview, and tabs route', async () => {
    mount('/customers/acme/bogus');
    await screen.findByRole('heading', { level: 1, name: 'Acme Corp' });
    expect(screen.getByRole('heading', { name: 'Cost by project' })).toBeInTheDocument();
    expect(await screen.findByText('nightly-review')).toBeInTheDocument();
    expect(getRuns).toHaveBeenCalledWith(expect.objectContaining({ customer: 'acme', limit: 10, host: 'local' }));

    fireEvent.click(screen.getByRole('button', { name: 'Projects' }));
    expect(screen.getByTestId('where')).toHaveTextContent('/customers/acme/projects');
    expect(await screen.findByText(/2 projects · runs in their subdirectories count too/)).toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: 'Overview' }));
    expect(screen.getByTestId('where')).toHaveTextContent(/^\/customers\/acme$/);

    for (const [name, path] of [
      ['Runs', 'runs'],
      ['Findings', 'findings'],
      ['Usage', 'usage'],
      ['Config', 'config'],
    ] as const) {
      fireEvent.click(screen.getByRole('button', { name }));
      expect(screen.getByTestId('where')).toHaveTextContent(`/customers/acme/${path}`);
    }
  });

  it('the Config tab mounts the layer editor and names its CLI commands', async () => {
    const getConfig = vi.spyOn(api, 'getCustomerConfig').mockResolvedValue({
      effective: { default_provider: 'acme-prod' },
      provenance: { default_provider: { source: 'customer', locked: false } },
      raw_global: '',
      raw_project: null,
      raw_customer: 'default_provider = "acme-prod"\n',
      customer_lock: [],
      layer_error: null,
      cp: {},
      status: { bind: '127.0.0.1:7878', token_set: false, restart_required_keys: [] },
    });
    mount('/customers/acme/config');
    await screen.findByRole('heading', { level: 1, name: 'Acme Corp' });
    expect(await screen.findByLabelText('Default provider')).toHaveValue('acme-prod');
    expect(getConfig).toHaveBeenCalledWith('acme');
    expect(screen.getByText(/applies to Acme Corp's 2 projects/)).toBeInTheDocument();
    expect(screen.getByText('rupu customer edit acme')).toBeInTheDocument();
    expect(screen.getByText('rupu customer show acme')).toBeInTheDocument();
  });
});

describe('CustomerDetail projects tab', () => {
  it('Unassign calls the API and refetches', async () => {
    const unassign = vi.spyOn(api, 'unassignProject').mockResolvedValue(undefined);
    mount('/customers/acme/projects');
    await screen.findByText('proj-a');
    const before = getCustomer.mock.calls.length;
    fireEvent.click(screen.getByRole('button', { name: 'Unassign proj-a' }));
    await waitFor(() => expect(unassign).toHaveBeenCalledWith('acme', 'a'));
    await waitFor(() => expect(getCustomer.mock.calls.length).toBeGreaterThan(before));
    expect(reload).toHaveBeenCalled();
  });

  it('a 404 on Unassign says "Already unassigned" and refetches', async () => {
    vi.spyOn(api, 'unassignProject').mockRejectedValue(new ApiError(404, 'nf', '{"error":"not assigned"}'));
    mount('/customers/acme/projects');
    await screen.findByText('proj-a');
    const before = getCustomer.mock.calls.length;
    fireEvent.click(screen.getByRole('button', { name: 'Unassign proj-a' }));
    expect(await screen.findByText(/Already unassigned/)).toBeInTheDocument();
    await waitFor(() => expect(getCustomer.mock.calls.length).toBeGreaterThan(before));
  });

  it('Assign project opens the dialog; assigning refetches', async () => {
    vi.spyOn(api, 'getProjects').mockResolvedValue([proj('c', 0, null)]);
    const assign = vi.spyOn(api, 'assignProject').mockResolvedValue(proj('c', 0));
    mount('/customers/acme/projects');
    await screen.findByText('proj-a');
    const before = getCustomer.mock.calls.length;
    fireEvent.click(screen.getByRole('button', { name: 'Assign project' }));
    const dialog = await screen.findByRole('dialog', { name: /Assign a project to Acme Corp/ });
    fireEvent.click(await within(dialog).findByRole('button', { name: /proj-c/ }));
    await waitFor(() => expect(assign).toHaveBeenCalledWith('acme', 'c'));
    await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument());
    await waitFor(() => expect(getCustomer.mock.calls.length).toBeGreaterThan(before));
  });

  it('an archived customer cannot be assigned projects', async () => {
    getCustomer.mockResolvedValue(detail({ customer: { ...detail().customer, archived: true } }));
    mount('/customers/acme/projects');
    await screen.findByText('proj-a');
    expect(screen.getByRole('button', { name: 'Assign project' })).toBeDisabled();
  });
});

describe('CustomerDetail delete', () => {
  function openDelete() {
    fireEvent.click(screen.getByRole('button', { name: 'More actions' }));
    fireEvent.click(screen.getByRole('menuitem', { name: 'Delete customer…' }));
    return screen.getByRole('dialog', { name: /Delete Acme Corp/ });
  }

  it('the kebab menu closes on Escape and returns focus to its trigger', async () => {
    mount();
    await screen.findByRole('heading', { level: 1, name: 'Acme Corp' });
    const trigger = screen.getByRole('button', { name: 'More actions' });
    fireEvent.click(trigger);
    const menu = screen.getByRole('menu');
    expect(within(menu).getByRole('menuitem', { name: 'Delete customer…' })).toHaveFocus();
    fireEvent.keyDown(menu, { key: 'Escape' });
    expect(screen.queryByRole('menu')).not.toBeInTheDocument();
    expect(trigger).toHaveFocus();
  });

  it('the delete dialog names the real customer directory', async () => {
    getCustomer.mockResolvedValue(detail({ config_path: '/srv/rupu-home/customers/acme/config.toml' }));
    mount();
    await screen.findByRole('heading', { level: 1, name: 'Acme Corp' });
    const dialog = openDelete();
    expect(within(dialog).getByText('/srv/rupu-home/customers/acme/')).toBeInTheDocument();
  });

  it('deleting with no projects removes the customer and returns to the list', async () => {
    const del = vi.spyOn(api, 'deleteCustomer').mockResolvedValue(undefined);
    mount();
    await screen.findByRole('heading', { level: 1, name: 'Acme Corp' });
    const dialog = openDelete();
    fireEvent.click(within(dialog).getByRole('button', { name: 'Delete' }));
    await waitFor(() => expect(del).toHaveBeenCalledWith('acme'));
    await waitFor(() => expect(screen.getByTestId('where')).toHaveTextContent(/^\/customers$/));
    expect(reload).toHaveBeenCalled();
    // The deleted customer was the scope: it is cleared.
    expect(setScope).toHaveBeenCalledWith(null);
  });

  it('a 409 lists the projects; "Unassign all and delete" runs the sequence', async () => {
    const conflict = new ApiError(
      409,
      'conflict',
      JSON.stringify({
        error: 'customer acme still has 2 projects',
        projects: [
          { ws_id: 'a', path: '/src/a' },
          { ws_id: 'b', path: '/src/b' },
        ],
      }),
    );
    const del = vi.spyOn(api, 'deleteCustomer');
    const order: string[] = [];
    const unassign = vi.spyOn(api, 'unassignProject').mockImplementation(async (_s, ws) => {
      order.push(`unassign:${ws}`);
    });
    del.mockImplementation(async () => {
      order.push('delete');
      if (order.filter((o) => o === 'delete').length === 1) throw conflict;
    });

    mount();
    await screen.findByRole('heading', { level: 1, name: 'Acme Corp' });
    const dialog = openDelete();
    fireEvent.click(within(dialog).getByRole('button', { name: 'Delete' }));
    expect(await within(dialog).findByText('/src/a')).toBeInTheDocument();
    expect(within(dialog).getByText('/src/b')).toBeInTheDocument();
    expect(within(dialog).getByText('customer acme still has 2 projects')).toBeInTheDocument();
    // Nothing was unassigned automatically.
    expect(unassign).not.toHaveBeenCalled();

    fireEvent.click(within(dialog).getByRole('button', { name: 'Unassign all and delete' }));
    await waitFor(() => expect(order).toEqual(['delete', 'unassign:a', 'unassign:b', 'delete']));
    await waitFor(() => expect(screen.getByTestId('where')).toHaveTextContent(/^\/customers$/));
  });
  it('a part-failed "Unassign all and delete" names what changed and refetches on close', async () => {
    const conflict = new ApiError(
      409,
      'conflict',
      JSON.stringify({
        error: '',
        projects: [
          { ws_id: 'a', path: '/src/a' },
          { ws_id: 'b', path: '/src/b' },
        ],
      }),
    );
    const del = vi.spyOn(api, 'deleteCustomer').mockRejectedValue(conflict);
    vi.spyOn(api, 'unassignProject').mockImplementation(async (_s, ws) => {
      if (ws === 'b') throw new ApiError(500, 'boom', JSON.stringify({ error: 'disk full' }));
    });

    mount();
    await screen.findByRole('heading', { level: 1, name: 'Acme Corp' });
    const dialog = openDelete();
    fireEvent.click(within(dialog).getByRole('button', { name: 'Delete' }));
    await within(dialog).findByText('/src/a');
    // An empty error string renders no paragraph.
    expect(dialog.querySelector('p.text-err')).toBeNull();

    fireEvent.click(within(dialog).getByRole('button', { name: 'Unassign all and delete' }));
    expect(
      await within(dialog).findByText('Unassigned /src/a, then failed on /src/b: disk full'),
    ).toBeInTheDocument();
    expect(del).toHaveBeenCalledTimes(1);
    // Only what is left stays listed.
    expect(within(dialog).queryByText('/src/a')).not.toBeInTheDocument();
    expect(within(dialog).getByText('/src/b')).toBeInTheDocument();

    const before = getCustomer.mock.calls.length;
    reload.mockClear();
    fireEvent.click(within(dialog).getByRole('button', { name: 'Cancel' }));
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    await waitFor(() => expect(getCustomer.mock.calls.length).toBeGreaterThan(before));
    expect(reload).toHaveBeenCalled();
  });
});

const AGENT_ROW: AgentRunRow = {
  run_id: 'agent_1',
  agent: 'reviewer',
  status: 'completed',
  started_at: '2026-10-05T00:00:00Z',
  source: 'standalone',
  usage: USAGE,
  customer: 'acme',
  host_id: 'local',
} as unknown as AgentRunRow;

const FINDING = {
  codename: 'cobalt-harbor/heron#1',
  codename_derived: false,
  id: 'f1',
  scope: null,
  summary: 'SQL injection in login',
  severity: 'high',
  evidence: { rationale: '' },
  declared_by: null,
  declared_at: '2026-07-01T00:00:00Z',
  ws_id: 'a',
  project: 'proj-a',
  target_id: 'src/login.rs',
  customer: 'acme',
} as FindingOut;

describe('CustomerDetail Runs tab', () => {
  it('lists the customer\'s workflow runs through the per-host engine', async () => {
    mount('/customers/acme/runs');
    expect(await screen.findByText('nightly-review')).toBeInTheDocument();
    expect(getWorkflowRuns).toHaveBeenCalledWith(
      expect.objectContaining({ customer: 'acme', host: 'local', lifecycle: 'active' }),
    );
    // Embedded: the page's own heading and the Archived state are not here.
    expect(screen.queryByRole('heading', { name: 'Workflow Runs' })).not.toBeInTheDocument();
    expect(screen.queryByText('Archived')).not.toBeInTheDocument();
  });

  it('the Agents toggle lists agent runs with the same customer', async () => {
    getAgentRuns.mockResolvedValue([AGENT_ROW]);
    mount('/customers/acme/runs');
    await screen.findByText('nightly-review');
    fireEvent.click(screen.getByRole('button', { name: 'Agents' }));
    await waitFor(() =>
      expect(getAgentRuns).toHaveBeenCalledWith(expect.objectContaining({ customer: 'acme', host: 'local' })),
    );
    expect(await screen.findByText('reviewer')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Workflows' }));
    expect(await screen.findByText('nightly-review')).toBeInTheDocument();
  });

  it('a remote host that cannot filter by customer shows as not included, never as zero', async () => {
    vi.spyOn(api, 'getRegisteredHosts').mockResolvedValue([REG_LOCAL, REG_PROD]);
    getWorkflowRuns.mockImplementation((p?: { host?: string }) =>
      p?.host === 'host_prod'
        ? Promise.reject(new ApiError(501, "host can't report a customer for every run", ''))
        : onlyHost('local', [RUN])(p),
    );
    mount('/customers/acme/runs');
    expect(await screen.findByText('nightly-review')).toBeInTheDocument();
    expect(await screen.findByText(/not included: prod/)).toBeInTheDocument();
    expect(callsFor(getWorkflowRuns, 'host_prod')[0][0]).toEqual(expect.objectContaining({ customer: 'acme' }));
    // …and the warn banner says why it is left out.
    expect(screen.getByTestId('hosts-without-customer')).toHaveTextContent(
      /prod runs an older rupu .* can’t tag every run with a customer — those runs are left out of this view, not counted as zero/,
    );
    // The archive can't be filtered by customer: a one-line hint says it isn't listed.
    expect(screen.getByText(/Archived runs aren’t listed here/)).toBeInTheDocument();
  });

  it('names the hosts a run listing\'s X-Rupu-Hosts-Without-Customer header lists', async () => {
    getWorkflowRuns.mockImplementation(
      (p?: { host?: string; onHostsWithoutCustomer?: (ids: string[]) => void }) => {
        p?.onHostsWithoutCustomer?.(['worker-7']);
        return Promise.resolve([RUN]);
      },
    );
    mount('/customers/acme/runs');
    expect(await screen.findByText('nightly-review')).toBeInTheDocument();
    expect(await screen.findByTestId('hosts-without-customer')).toHaveTextContent(/worker-7 runs an older rupu/);
  });

  it('marks a run whose cost was priced at the global rates', async () => {
    getWorkflowRuns.mockResolvedValue([{ ...RUN, usage: { ...USAGE, cost_usd: 1, pricing_error: 'acme layer broken' } }]);
    mount('/customers/acme/runs');
    await screen.findByText('nightly-review');
    // The row-link cells are aria-hidden (mouse-only), so find the mark by its tooltip.
    expect(screen.getAllByTitle('acme layer broken').length).toBeGreaterThan(0);
  });
});

describe('CustomerDetail Findings tab', () => {
  it('loads the findings of the customer\'s projects', async () => {
    getFindings.mockResolvedValue({
      findings: [FINDING],
      summary: { total: 1, critical: 0, high: 1, medium: 0, low: 0, info: 0 },
    });
    mount('/customers/acme/findings');
    expect(await screen.findByText('SQL injection in login')).toBeInTheDocument();
    expect(getFindings).toHaveBeenCalledWith({ customer: 'acme' });
    expect(screen.queryByRole('heading', { name: 'Findings' })).not.toBeInTheDocument();
  });

  it('says so when none of its projects has a finding', async () => {
    mount('/customers/acme/findings');
    expect(await screen.findByText('No findings')).toBeInTheDocument();
    expect(getFindings).toHaveBeenCalledWith({ customer: 'acme' });
  });
});

describe('CustomerDetail Usage tab', () => {
  it('fetches every usage source with the customer', async () => {
    mount('/customers/acme/usage');
    await waitFor(() =>
      expect(getUsage).toHaveBeenCalledWith(expect.anything(), 'model', 'local', expect.any(AbortSignal), 'acme'),
    );
    // The trailing sink receives the `X-Rupu-Hosts-Without-Customer` header.
    await waitFor(() =>
      expect(getUsageRuns).toHaveBeenCalledWith(expect.anything(), undefined, 'acme', expect.any(Function)),
    );
    await waitFor(() => expect(getUsageOutliers).toHaveBeenCalledWith(expect.anything(), 'acme', expect.any(Function)));
    expect(await screen.findByText('$7.50')).toBeInTheDocument();
    expect(screen.getByText(/Breakdown by/)).toBeInTheDocument();
    expect(screen.queryByRole('heading', { name: 'Usage' })).not.toBeInTheDocument();
  });

  it('a remote host answers 501 under the filter and shows as unavailable, not counted', async () => {
    vi.spyOn(api, 'getRegisteredHosts').mockResolvedValue([REG_LOCAL, REG_PROD]);
    getUsage.mockImplementation((_w: unknown, _p: unknown, host?: string) =>
      host === 'host_prod'
        ? Promise.reject(
            new ApiError(
              501,
              'x',
              JSON.stringify({ error: "host host_prod can't be filtered by customer: its totals are summed remotely" }),
            ),
          )
        : Promise.resolve(USAGE_RESPONSE),
    );
    mount('/customers/acme/usage');
    expect(await screen.findByText('$7.50')).toBeInTheDocument();
    expect(await screen.findByText(/excludes prod \(unavailable\)/)).toBeInTheDocument();
    expect(screen.getByTestId('hosts-without-customer')).toHaveTextContent(
      /prod can’t be filtered by customer \(its totals are summed on the host\)/,
    );
  });

  it('marks a headline cost priced at the global rates', async () => {
    getUsage.mockResolvedValue({
      ...USAGE_RESPONSE,
      summary: { ...USAGE_RESPONSE.summary, pricing_error: 'acme layer broken' },
    });
    mount('/customers/acme/usage');
    await screen.findByText('$7.50');
    expect(screen.getByRole('img', { name: /Pricing unavailable: acme layer broken/ })).toBeInTheDocument();
  });
});
