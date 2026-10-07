// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { api, ApiError, type CustomerRef, type ProjectRow } from '../../lib/api';
import { AssignProjectDialog } from './AssignProjectDialog';

const USAGE = {
  input_tokens: 0,
  output_tokens: 0,
  cached_tokens: 0,
  total_tokens: 0,
  cost_usd: 0,
  priced: true,
  runs: 0,
};
const TINT = { light: '#111111', dark: '#eeeeee' };
const ACME: CustomerRef = { slug: 'acme', name: 'Acme Corp', tint: TINT, archived: false };
const GLOBEX: CustomerRef = { slug: 'globex', name: 'Globex', tint: TINT, archived: false };

function proj(ws: string, customer: ProjectRow['customer'] | undefined): ProjectRow {
  const p = {
    ws_id: ws,
    name: `proj-${ws}`,
    path: `/src/${ws}`,
    created_at: '2026-01-01T00:00:00Z',
    usage: USAGE,
    run_count: 0,
    customer,
  } as ProjectRow;
  if (customer === undefined) delete (p as Partial<ProjectRow>).customer;
  return p;
}

const PROJECTS = [proj('free1', null), proj('free2', null), proj('mine', ACME), proj('theirs', GLOBEX), proj('unk', undefined)];

let onAssigned: ReturnType<typeof vi.fn<(p: ProjectRow) => void>>;
let onClose: ReturnType<typeof vi.fn<() => void>>;
let assign: ReturnType<typeof vi.spyOn>;

function mount() {
  return render(
    <AssignProjectDialog customer={ACME} onAssigned={onAssigned} onClose={onClose} />,
  );
}

beforeEach(() => {
  onAssigned = vi.fn<(p: ProjectRow) => void>();
  onClose = vi.fn<() => void>();
  vi.spyOn(api, 'getProjects').mockResolvedValue(PROJECTS);
  assign = vi.spyOn(api, 'assignProject').mockImplementation(async (_s, ws) => proj(ws, ACME));
});
afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

function optionNames() {
  return within(screen.getByRole('list', { name: 'Projects' }))
    .getAllByRole('button')
    .map((b) => b.getAttribute('data-ws'));
}

describe('AssignProjectDialog', () => {
  it('lists only the projects with no customer', async () => {
    mount();
    expect(screen.getByRole('dialog', { name: 'Assign a project to Acme Corp' })).toBeInTheDocument();
    await screen.findByText('proj-free1');
    expect(optionNames()).toEqual(['free1', 'free2']);
  });

  it('picking an unassigned project assigns it', async () => {
    mount();
    fireEvent.click(await screen.findByRole('button', { name: /proj-free2/ }));
    await waitFor(() => expect(assign).toHaveBeenCalledWith('acme', 'free2'));
    await waitFor(() => expect(onAssigned).toHaveBeenCalled());
  });

  it('the filter narrows by name or path', async () => {
    mount();
    await screen.findByText('proj-free1');
    fireEvent.change(screen.getByRole('textbox', { name: 'Find a project' }), { target: { value: 'free2' } });
    expect(optionNames()).toEqual(['free2']);
  });

  it('"Show projects of other customers" lists the rest with their chip; moving needs a confirm click', async () => {
    mount();
    await screen.findByText('proj-free1');
    fireEvent.click(screen.getByRole('checkbox', { name: 'Show projects of other customers' }));
    // Never this customer's own projects.
    expect(optionNames()).toEqual(['free1', 'free2', 'theirs', 'unk']);
    const theirs = screen.getByRole('button', { name: /proj-theirs/ });
    expect(theirs).toHaveTextContent('Globex');
    expect(screen.getByRole('button', { name: /proj-unk/ })).toHaveTextContent('Unknown customer');

    fireEvent.click(theirs);
    expect(assign).not.toHaveBeenCalled();
    expect(screen.getByText('Moves proj-theirs from Globex to Acme Corp')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Move' }));
    await waitFor(() => expect(assign).toHaveBeenCalledWith('acme', 'theirs'));
    await waitFor(() => expect(onAssigned).toHaveBeenCalled());
  });

  it('an assignment the API refuses is shown inline', async () => {
    assign.mockRejectedValue(new ApiError(409, 'x', JSON.stringify({ error: 'customer acme is archived' })));
    mount();
    fireEvent.click(await screen.findByRole('button', { name: /proj-free1/ }));
    expect(await screen.findByText('customer acme is archived')).toBeInTheDocument();
    expect(onAssigned).not.toHaveBeenCalled();
  });

  it('Escape closes', async () => {
    mount();
    await screen.findByText('proj-free1');
    fireEvent.keyDown(document, { key: 'Escape' });
    expect(onClose).toHaveBeenCalled();
  });

  it('says so when every project has a customer', async () => {
    vi.spyOn(api, 'getProjects').mockResolvedValue([proj('mine', ACME)]);
    mount();
    expect(await screen.findByText(/Every project already has a customer/)).toBeInTheDocument();
  });
});
