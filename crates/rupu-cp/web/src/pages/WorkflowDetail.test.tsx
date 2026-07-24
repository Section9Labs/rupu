// @vitest-environment jsdom
// WorkflowDetail edit/delete — the operator edits a workflow's `.yaml`
// definition in the single-screen unified editor (graph + live YAML), saves it
// (validated server-side), or deletes the workflow.
//
// `WorkflowEditor` is mocked to a stub button so the real (lazy) editor — and
// with it @xyflow/react + CodeMirror — never loads; the stub emits a fresh YAML
// through `onYamlChange`, exercising the draft-sync + Save wiring. `useNavigate`
// is mocked to assert navigation.

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

// Mock the (lazy) unified editor to a stub — keeps @xyflow/react + CodeMirror
// out of the test. The stub button emits a fresh YAML through onYamlChange,
// exercising the draft-sync + Save wiring without the real shell.
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

describe('WorkflowDetail', () => {
  it('loads and renders the (stubbed) unified editor', async () => {
    vi.spyOn(api, 'getWorkflow').mockResolvedValue(DETAIL);
    renderPage();

    expect(await screen.findByTestId('stub-editor')).toBeInTheDocument();
    // Header still shows the workflow name.
    expect(screen.getByRole('heading', { name: 'nightly' })).toBeInTheDocument();
  });

  it('an onYamlChange from the editor makes the workflow dirty → Save enabled → saves', async () => {
    vi.spyOn(api, 'getWorkflow').mockResolvedValue(DETAIL);
    const saveSpy = vi
      .spyOn(api, 'saveWorkflow')
      .mockResolvedValue({ ...DETAIL, yaml: STUB_YAML });
    renderPage();

    const saveBtn = await screen.findByRole('button', { name: 'Save' });
    // No edits yet → draft matches saved YAML → Save disabled.
    expect(saveBtn).toBeDisabled();

    // Emit a fresh YAML from the (stub) editor → draft diverges.
    fireEvent.click(await screen.findByTestId('stub-editor'));
    await waitFor(() => expect(saveBtn).not.toBeDisabled());

    fireEvent.click(saveBtn);
    await waitFor(() => expect(saveSpy).toHaveBeenCalledWith('nightly', STUB_YAML));
    // On success the draft re-syncs to the saved YAML → Save disabled again.
    await waitFor(() => expect(screen.getByRole('button', { name: 'Save' })).toBeDisabled());
  });

  it('surfaces a rejected Save as an inline alert', async () => {
    vi.spyOn(api, 'getWorkflow').mockResolvedValue(DETAIL);
    vi.spyOn(api, 'saveWorkflow').mockRejectedValue(
      new ApiError(400, 'invalid workflow: missing prompt'),
    );
    renderPage();

    fireEvent.click(await screen.findByTestId('stub-editor'));
    const saveBtn = await screen.findByRole('button', { name: 'Save' });
    await waitFor(() => expect(saveBtn).not.toBeDisabled());
    fireEvent.click(saveBtn);

    const alert = await screen.findByRole('alert');
    expect(alert).toHaveTextContent('invalid workflow: missing prompt');
  });

  it('an invalid validateWorkflow result shows the reason and disables Save', async () => {
    vi.spyOn(api, 'getWorkflow').mockResolvedValue(DETAIL);
    vi.spyOn(api, 'validateWorkflow').mockResolvedValue({
      ok: false,
      error: 'invalid workflow: unknown agent',
    });
    renderPage();

    // Diverge the draft so the only thing keeping Save disabled is invalidity.
    fireEvent.click(await screen.findByTestId('stub-editor'));

    // The debounced badge surfaces the server's reason…
    expect(await screen.findByText(/invalid workflow: unknown agent/)).toBeInTheDocument();
    // …and Save stays disabled while invalid.
    expect(screen.getByRole('button', { name: 'Save' })).toBeDisabled();
  });

  it('registers a beforeunload guard once the draft is dirty', async () => {
    vi.spyOn(api, 'getWorkflow').mockResolvedValue(DETAIL);
    const addSpy = vi.spyOn(window, 'addEventListener');
    renderPage();

    await screen.findByTestId('stub-editor');
    // Clean draft → no beforeunload guard yet.
    expect(addSpy.mock.calls.some(([type]) => type === 'beforeunload')).toBe(false);

    // Diverge the draft → the guard effect registers the listener.
    fireEvent.click(screen.getByTestId('stub-editor'));
    await waitFor(() =>
      expect(addSpy.mock.calls.some(([type]) => type === 'beforeunload')).toBe(true),
    );
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

  // ── Autoflow enable/disable ──────────────────────────────────────────────

  const ENABLED_AUTOFLOW: WorkflowDetail = {
    ...DETAIL,
    workflow: { ...DETAIL.workflow, autoflow: { enabled: true } },
  };

  const DISABLED_AUTOFLOW: WorkflowDetail = {
    ...DETAIL,
    workflow: { ...DETAIL.workflow, autoflow: { enabled: false } },
  };

  it('an enabled autoflow shows Disable; clicking calls /disable and reflects disabled', async () => {
    vi.spyOn(api, 'getWorkflow').mockResolvedValue(ENABLED_AUTOFLOW);
    const setSpy = vi
      .spyOn(api, 'setAutoflowEnabled')
      .mockResolvedValue({ name: 'nightly', enabled: false });
    renderPage();

    const disableBtn = await screen.findByRole('button', { name: 'Disable nightly' });
    expect(screen.queryByRole('button', { name: 'Resume nightly' })).not.toBeInTheDocument();

    fireEvent.click(disableBtn);

    await waitFor(() => expect(setSpy).toHaveBeenCalledWith('nightly', false));
    expect(await screen.findByRole('button', { name: 'Resume nightly' })).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Disable nightly' })).not.toBeInTheDocument();
  });

  it('a disabled autoflow shows Resume; clicking calls /enable and reflects enabled', async () => {
    vi.spyOn(api, 'getWorkflow').mockResolvedValue(DISABLED_AUTOFLOW);
    const setSpy = vi
      .spyOn(api, 'setAutoflowEnabled')
      .mockResolvedValue({ name: 'nightly', enabled: true });
    renderPage();

    const resumeBtn = await screen.findByRole('button', { name: 'Resume nightly' });
    expect(screen.queryByRole('button', { name: 'Disable nightly' })).not.toBeInTheDocument();

    fireEvent.click(resumeBtn);

    await waitFor(() => expect(setSpy).toHaveBeenCalledWith('nightly', true));
    expect(await screen.findByRole('button', { name: 'Disable nightly' })).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Resume nightly' })).not.toBeInTheDocument();
  });

  it('a 501 (read-only deploy) renders a clear message and leaves state unchanged', async () => {
    vi.spyOn(api, 'getWorkflow').mockResolvedValue(ENABLED_AUTOFLOW);
    vi.spyOn(api, 'setAutoflowEnabled').mockRejectedValue(
      new ApiError(501, 'enabling/disabling an autoflow requires `rupu cp serve`'),
    );
    renderPage();

    fireEvent.click(await screen.findByRole('button', { name: 'Disable nightly' }));

    await screen.findByText(/requires/i);
    expect(screen.getByText(/rupu cp serve/)).toBeInTheDocument();
    // State is unchanged — the button still reads Disable (still enabled).
    expect(screen.getByRole('button', { name: 'Disable nightly' })).toBeInTheDocument();
  });

  it('a non-autoflow workflow shows neither Disable nor Resume', async () => {
    vi.spyOn(api, 'getWorkflow').mockResolvedValue(DETAIL);
    renderPage();

    await screen.findByRole('heading', { name: 'nightly' });
    expect(screen.queryByRole('button', { name: 'Disable nightly' })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Resume nightly' })).not.toBeInTheDocument();
  });

  // ── Client-side gate on the server /validate POST ────────────────────────
  // A structurally incomplete draft (branch with no condition, step with no
  // agent/prompt, etc.) must never reach the server — the backend parser 400s
  // on it, and re-triggering that on every keystroke floods the console. The
  // client runs `validateGraph(yamlToGraph(...))` first and only POSTs once
  // the graph is structurally clean.

  const INCOMPLETE_BRANCH_YAML = 'name: w\nsteps:\n  - id: b\n    branch: {}\n';
  const COMPLETE_STEP_YAML = 'name: w\nsteps:\n  - id: a\n    agent: x\n    prompt: p\n';

  it('does not POST to /validate while a node is structurally incomplete', async () => {
    vi.spyOn(api, 'getWorkflow').mockResolvedValue({
      workflow: { name: 'w', steps: [] },
      yaml: INCOMPLETE_BRANCH_YAML,
    });
    const spy = vi.spyOn(api, 'validateWorkflow');
    renderPage();

    await screen.findByTestId('stub-editor');
    // Give the 400ms debounce time to fire, then assert it never called out.
    await new Promise((r) => setTimeout(r, 600));
    expect(spy).not.toHaveBeenCalled();
    // The inline client-side reason renders instead of a server error.
    expect(await screen.findByText(/unfinished/i)).toBeInTheDocument();
  });

  it('DOES POST once the graph is structurally complete', async () => {
    vi.spyOn(api, 'getWorkflow').mockResolvedValue({
      workflow: { name: 'w', steps: [{ id: 'a', agent: 'x', prompt: 'p' }] },
      yaml: COMPLETE_STEP_YAML,
    });
    const spy = vi.spyOn(api, 'validateWorkflow').mockResolvedValue({ ok: true });
    renderPage();

    await screen.findByTestId('stub-editor');
    await waitFor(() => expect(spy).toHaveBeenCalledTimes(1));
    expect(spy).toHaveBeenCalledWith(COMPLETE_STEP_YAML);
  });
});
