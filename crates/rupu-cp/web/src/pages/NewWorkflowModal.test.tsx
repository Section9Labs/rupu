// @vitest-environment jsdom
// NewWorkflowModal — Describe/Edit toggle + AI generate.
// Opens the modal via "New workflow", switches to Describe mode, types a prompt,
// clicks Generate, and asserts the generated raw definition loads into the editor.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { render, screen, cleanup, fireEvent, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { api, ApiError } from '../lib/api';
import { launchApi } from '../lib/api3b/launch';

const navigateMock = vi.fn();
vi.mock('react-router-dom', async () => {
  const actual = await vi.importActual<typeof import('react-router-dom')>('react-router-dom');
  return { ...actual, useNavigate: () => navigateMock };
});

vi.mock('../components/charts/UsageBarChart', () => ({
  __esModule: true,
  default: () => <div data-testid="usage-bar-chart" />,
}));

vi.mock('../components/LauncherSheet', () => ({
  __esModule: true,
  default: () => <div data-testid="launcher-sheet" />,
}));

vi.mock('../components/CodeEditor', () => ({
  __esModule: true,
  default: ({
    value,
    onChange,
    ariaLabel,
  }: {
    value: string;
    onChange: (v: string) => void;
    ariaLabel?: string;
  }) => (
    <textarea
      data-testid="code-editor"
      aria-label={ariaLabel}
      value={value}
      onChange={(e) => onChange(e.target.value)}
    />
  ),
}));

import Workflows from './Workflows';
import { withCustomerScope } from '../lib/customerScopeTestUtils';

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  navigateMock.mockReset();
});

describe('NewWorkflowModal describe mode', () => {
  it('generates a draft into the editor', async () => {
    vi.spyOn(api, 'getWorkflows').mockResolvedValue([]);
    vi.spyOn(api, 'generateModels').mockResolvedValue([
      { provider: 'anthropic', models: ['claude-sonnet-4-6'], is_default: true },
    ]);
    const gen = vi.spyOn(api, 'generateWorkflow').mockResolvedValue({
      raw: 'name: drafted-wf',
      provider: 'anthropic',
      model: 'claude-sonnet-4-6',
      attempts: 1,
    });

    render(
      <MemoryRouter>
        {withCustomerScope(<Workflows />)}
      </MemoryRouter>,
    );

    fireEvent.click(await screen.findByRole('button', { name: /new workflow/i }));
    fireEvent.click(await screen.findByRole('button', { name: /describe/i }));
    fireEvent.change(screen.getByLabelText(/describe the workflow/i), {
      target: { value: 'review then fix' },
    });
    fireEvent.click(screen.getByRole('button', { name: /generate/i }));

    await waitFor(() => expect(gen).toHaveBeenCalled());
    expect(await screen.findByDisplayValue(/name: drafted-wf/)).toBeInTheDocument();
  });
});

describe('NewWorkflowModal run without saving (Plan 3b)', () => {
  async function openEditor() {
    vi.spyOn(api, 'getWorkflows').mockResolvedValue([]);
    vi.spyOn(api, 'generateModels').mockResolvedValue([]);
    render(
      <MemoryRouter>
        {withCustomerScope(<Workflows />)}
      </MemoryRouter>,
    );
    fireEvent.click(await screen.findByRole('button', { name: /new workflow/i }));
    return screen.findByLabelText('New workflow definition');
  }

  it('launches the YAML as typed, with its declared inputs and the mode, and opens the run', async () => {
    const run = vi.spyOn(launchApi, 'runWorkflowYaml').mockResolvedValue({ run_id: 'run_y', file: '/f.yaml' });
    const create = vi.spyOn(api, 'createWorkflow');
    const editor = await openEditor();
    const yamlText = 'name: once\ninputs:\n  target:\n    required: true\nsteps:\n  - id: a\n    agent: x\n    prompt: go\n';
    fireEvent.change(editor, { target: { value: yamlText } });
    fireEvent.change(await screen.findByLabelText('Input target'), { target: { value: 'src/' } });
    fireEvent.change(screen.getByLabelText('Run mode'), { target: { value: 'readonly' } });
    fireEvent.click(screen.getByRole('button', { name: 'Run without saving' }));

    await waitFor(() =>
      expect(run).toHaveBeenCalledWith({ yaml: yamlText, mode: 'readonly', inputs: { target: 'src/' } }),
    );
    await waitFor(() => expect(navigateMock).toHaveBeenCalledWith('/runs/run_y'));
    expect(create).not.toHaveBeenCalled();
  });

  it('shows the server’s refusal under the button', async () => {
    const body = '{"error":"workflow does not parse: missing field `steps`"}';
    vi.spyOn(launchApi, 'runWorkflowYaml').mockRejectedValue(new ApiError(400, body, body));
    await openEditor();
    fireEvent.click(screen.getByRole('button', { name: 'Run without saving' }));
    expect(await screen.findByText('workflow does not parse: missing field `steps`')).toBeInTheDocument();
    expect(navigateMock).not.toHaveBeenCalled();
  });
});
