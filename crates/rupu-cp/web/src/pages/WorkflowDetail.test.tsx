// @vitest-environment jsdom
// WorkflowDetail edit/delete — the operator edits a workflow's `.yaml`
// definition in the browser (YAML tab) or visually (Graph tab), saves it
// (validated server-side), or deletes the workflow.
//
// `CodeEditor` is mocked to a plain <textarea> so the test never pulls in the
// real (lazy) CodeMirror chunk; `WorkflowEditor` is mocked to a stub button so
// @xyflow/react never loads; `useNavigate` is mocked to assert navigation.

import '@testing-library/jest-dom/vitest';
import { afterEach, beforeEach, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, fireEvent, waitFor } from '@testing-library/react';
import { MemoryRouter, Route, Routes } from 'react-router-dom';
import { api, ApiError, type WorkflowDetail } from '../lib/api';

const navigateMock = vi.fn();
vi.mock('react-router-dom', async () => {
  const actual = await vi.importActual<typeof import('react-router-dom')>('react-router-dom');
  return { ...actual, useNavigate: () => navigateMock };
});

// Mock CodeEditor to a controlled <textarea> — keeps CodeMirror out of the test.
vi.mock('../components/CodeEditor', () => ({
  __esModule: true,
  default: ({ value, onChange, ariaLabel }: { value: string; onChange: (v: string) => void; ariaLabel?: string }) => (
    <textarea
      data-testid="code-editor"
      aria-label={ariaLabel}
      value={value}
      onChange={(e) => onChange(e.target.value)}
    />
  ),
}));

// Mock the (lazy) visual editor to a stub — keeps @xyflow/react out of the test.
// The stub button emits a fresh YAML through onYamlChange, exercising the
// draft-sync + Save wiring without the real canvas.
const STUB_YAML = 'name: x\nsteps: []\n';
vi.mock('../components/workflow-editor/WorkflowEditor', () => ({
  __esModule: true,
  default: ({ onYamlChange }: { onYamlChange: (y: string) => void }) => (
    <button type="button" data-testid="stub-editor" onClick={() => onYamlChange(STUB_YAML)}>
      emit
    </button>
  ),
}));

import WorkflowDetailPage from './WorkflowDetail';

const YAML = `name: nightly\ndescription: Nightly sweep.\nsteps:\n  - id: scan\n    agent: scanner\n    prompt: Scan the repo\n`;

const DETAIL: WorkflowDetail = {
  workflow: {
    name: 'nightly',
    scope: 'global',
    description: 'Nightly sweep.',
    steps: [{ id: 'scan', agent: 'scanner', prompt: 'Scan the repo' }],
  },
  yaml: YAML,
};

beforeEach(() => {
  // Defaults — individual tests override as needed. getAgents + validateWorkflow
  // are fired from effects on every render; stub them so they never hit fetch.
  vi.spyOn(api, 'getAgents').mockResolvedValue([]);
  vi.spyOn(api, 'validateWorkflow').mockResolvedValue({ ok: true });
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  navigateMock.mockReset();
});

function renderPage() {
  return render(
    <MemoryRouter initialEntries={['/workflows/nightly']}>
      <Routes>
        <Route path="/workflows/:name" element={<WorkflowDetailPage />} />
      </Routes>
    </MemoryRouter>,
  );
}

describe('WorkflowDetail edit/delete', () => {
  it('Edit reveals the editor seeded with the raw YAML', async () => {
    vi.spyOn(api, 'getWorkflow').mockResolvedValue(DETAIL);
    renderPage();

    fireEvent.click(await screen.findByRole('button', { name: 'Edit YAML' }));

    const editor = (await screen.findByTestId('code-editor')) as HTMLTextAreaElement;
    expect(editor.value).toBe(YAML);
  });

  it('editing + Save calls saveWorkflow(name, draft) and exits edit mode', async () => {
    vi.spyOn(api, 'getWorkflow').mockResolvedValue(DETAIL);
    const next = `${YAML}# trailing comment\n`;
    const saveSpy = vi
      .spyOn(api, 'saveWorkflow')
      .mockResolvedValue({ ...DETAIL, yaml: next });
    renderPage();

    fireEvent.click(await screen.findByRole('button', { name: 'Edit YAML' }));
    const editor = (await screen.findByTestId('code-editor')) as HTMLTextAreaElement;

    // Save disabled until the draft diverges from the saved YAML.
    const saveBtn = screen.getByRole('button', { name: 'Save' });
    expect(saveBtn).toBeDisabled();

    fireEvent.change(editor, { target: { value: next } });
    expect(saveBtn).not.toBeDisabled();
    fireEvent.click(saveBtn);

    await waitFor(() => expect(saveSpy).toHaveBeenCalledWith('nightly', next));
    // On success the editor closes (read-only highlight returns).
    await waitFor(() => expect(screen.queryByTestId('code-editor')).not.toBeInTheDocument());
  });

  it('surfaces a rejected Save as an inline alert', async () => {
    vi.spyOn(api, 'getWorkflow').mockResolvedValue(DETAIL);
    vi.spyOn(api, 'saveWorkflow').mockRejectedValue(
      new ApiError(400, 'invalid workflow: missing prompt'),
    );
    renderPage();

    fireEvent.click(await screen.findByRole('button', { name: 'Edit YAML' }));
    const editor = (await screen.findByTestId('code-editor')) as HTMLTextAreaElement;
    fireEvent.change(editor, { target: { value: `${YAML}x` } });
    fireEvent.click(screen.getByRole('button', { name: 'Save' }));

    const alert = await screen.findByRole('alert');
    expect(alert).toHaveTextContent('invalid workflow: missing prompt');
    // Stays in edit mode so the operator can fix it.
    expect(screen.getByTestId('code-editor')).toBeInTheDocument();
  });

  it('Delete (confirmed) calls deleteWorkflow and navigates to /workflows', async () => {
    vi.spyOn(api, 'getWorkflow').mockResolvedValue(DETAIL);
    const delSpy = vi.spyOn(api, 'deleteWorkflow').mockResolvedValue();
    vi.spyOn(window, 'confirm').mockReturnValue(true);
    renderPage();

    fireEvent.click(await screen.findByRole('button', { name: 'Delete nightly' }));

    await waitFor(() => expect(delSpy).toHaveBeenCalledWith('nightly'));
    expect(navigateMock).toHaveBeenCalledWith('/workflows');
  });

  it('Delete (cancelled at confirm) does nothing', async () => {
    vi.spyOn(api, 'getWorkflow').mockResolvedValue(DETAIL);
    const delSpy = vi.spyOn(api, 'deleteWorkflow').mockResolvedValue();
    vi.spyOn(window, 'confirm').mockReturnValue(false);
    renderPage();

    fireEvent.click(await screen.findByRole('button', { name: 'Delete nightly' }));
    expect(delSpy).not.toHaveBeenCalled();
    expect(navigateMock).not.toHaveBeenCalled();
  });
});

describe('WorkflowDetail Graph ⇄ YAML tabs', () => {
  it('switching to the Graph tab renders the (stubbed) visual editor', async () => {
    vi.spyOn(api, 'getWorkflow').mockResolvedValue(DETAIL);
    renderPage();

    fireEvent.click(await screen.findByRole('button', { name: 'Graph' }));
    expect(await screen.findByTestId('stub-editor')).toBeInTheDocument();
  });

  it('an onYamlChange from the editor updates the draft, enabling Save', async () => {
    vi.spyOn(api, 'getWorkflow').mockResolvedValue(DETAIL);
    const saveSpy = vi
      .spyOn(api, 'saveWorkflow')
      .mockResolvedValue({ ...DETAIL, yaml: STUB_YAML });
    renderPage();

    fireEvent.click(await screen.findByRole('button', { name: 'Graph' }));
    const saveBtn = await screen.findByRole('button', { name: 'Save' });
    // No edits yet → draft matches saved YAML → Save disabled.
    expect(saveBtn).toBeDisabled();

    // Emit a fresh YAML from the (stub) editor → draft diverges.
    fireEvent.click(screen.getByTestId('stub-editor'));
    await waitFor(() => expect(saveBtn).not.toBeDisabled());

    fireEvent.click(saveBtn);
    await waitFor(() => expect(saveSpy).toHaveBeenCalledWith('nightly', STUB_YAML));
  });

  it('an invalid validateWorkflow result shows the reason and disables Save', async () => {
    vi.spyOn(api, 'getWorkflow').mockResolvedValue(DETAIL);
    vi.spyOn(api, 'validateWorkflow').mockResolvedValue({
      ok: false,
      error: 'invalid workflow: unknown agent',
    });
    renderPage();

    fireEvent.click(await screen.findByRole('button', { name: 'Graph' }));
    // Diverge the draft so the only thing keeping Save disabled is invalidity.
    fireEvent.click(await screen.findByTestId('stub-editor'));

    // The debounced badge surfaces the server's reason…
    expect(await screen.findByText(/invalid workflow: unknown agent/)).toBeInTheDocument();
    // …and Save stays disabled while invalid.
    expect(screen.getByRole('button', { name: 'Save' })).toBeDisabled();
  });
});
