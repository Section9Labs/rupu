// @vitest-environment jsdom
// WorkflowDetail edit/delete — the operator edits a workflow's `.yaml`
// definition in the browser, saves it (validated server-side), or deletes the
// workflow.
//
// `CodeEditor` is mocked to a plain <textarea> so the test never pulls in the
// real (lazy) CodeMirror chunk; `useNavigate` is mocked to assert navigation.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect, vi } from 'vitest';
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
