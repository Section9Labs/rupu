// @vitest-environment jsdom
// AgentBuilder — the card-composer shell (Task 6): draft state seeded from
// `parseAgent(initialRaw)`, live `.md` preview (`data-testid="ab-yaml"`),
// and submit wired to `serializeAgent(draft)`. CodeEditor is mocked to a
// plain textarea (house pattern — see WorkflowEditor.test.tsx /
// NewAgentModal.test.tsx) so Raw mode doesn't pull in CodeMirror.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, fireEvent } from '@testing-library/react';

vi.mock('../CodeEditor', () => ({
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

import AgentBuilder from './AgentBuilder';

const SAMPLE_RAW = `---
name: security-reviewer
description: Structured security reviewer for panel workflows.
permissionMode: readonly
---

You are a senior application-security reviewer.
`;

afterEach(cleanup);

describe('AgentBuilder', () => {
  it('seeds the name field from the parsed draft', () => {
    render(
      <AgentBuilder
        initialRaw={SAMPLE_RAW}
        submitLabel="Create agent"
        submitting={false}
        error={null}
        onSubmit={vi.fn()}
      />,
    );
    expect(screen.getByLabelText('agent name')).toHaveValue('security-reviewer');
  });

  it('updates the live preview as the name changes', () => {
    render(
      <AgentBuilder
        initialRaw={SAMPLE_RAW}
        submitLabel="Create agent"
        submitting={false}
        error={null}
        onSubmit={vi.fn()}
      />,
    );
    fireEvent.change(screen.getByLabelText('agent name'), { target: { value: 'threat-modeler' } });
    expect(screen.getByTestId('ab-yaml')).toHaveTextContent('threat-modeler');
  });

  it('submits serializeAgent(draft) with the edited name on click', () => {
    const onSubmit = vi.fn();
    render(
      <AgentBuilder
        initialRaw={SAMPLE_RAW}
        submitLabel="Create agent"
        submitting={false}
        error={null}
        onSubmit={onSubmit}
      />,
    );
    fireEvent.change(screen.getByLabelText('agent name'), { target: { value: 'threat-modeler' } });
    fireEvent.click(screen.getByRole('button', { name: 'Create agent' }));
    expect(onSubmit).toHaveBeenCalledTimes(1);
    expect(onSubmit.mock.calls[0][0]).toContain('name: threat-modeler');
  });

  it('disables submit when the draft is invalid (empty name)', () => {
    render(
      <AgentBuilder
        initialRaw={SAMPLE_RAW}
        submitLabel="Create agent"
        submitting={false}
        error={null}
        onSubmit={vi.fn()}
      />,
    );
    fireEvent.change(screen.getByLabelText('agent name'), { target: { value: '' } });
    expect(screen.getByRole('button', { name: 'Create agent' })).toBeDisabled();
  });

  it('disables submit while submitting', () => {
    render(
      <AgentBuilder
        initialRaw={SAMPLE_RAW}
        submitLabel="Create agent"
        submitting
        error={null}
        onSubmit={vi.fn()}
      />,
    );
    expect(screen.getByRole('button', { name: 'Create agent' })).toBeDisabled();
  });

  it('shows the error message when present', () => {
    render(
      <AgentBuilder
        initialRaw={SAMPLE_RAW}
        submitLabel="Create agent"
        submitting={false}
        error="boom"
        onSubmit={vi.fn()}
      />,
    );
    expect(screen.getByText('boom')).toBeInTheDocument();
  });

  it('does not render an AI tab when onGenerate is omitted', () => {
    render(
      <AgentBuilder
        initialRaw={SAMPLE_RAW}
        submitLabel="Create agent"
        submitting={false}
        error={null}
        onSubmit={vi.fn()}
      />,
    );
    expect(screen.queryByRole('button', { name: /^AI/i })).not.toBeInTheDocument();
  });

  it('switches to Raw mode and edits the draft through the code editor', () => {
    const onSubmit = vi.fn();
    render(
      <AgentBuilder
        initialRaw={SAMPLE_RAW}
        submitLabel="Create agent"
        submitting={false}
        error={null}
        onSubmit={onSubmit}
      />,
    );
    fireEvent.click(screen.getByRole('button', { name: 'Raw' }));
    const editor = screen.getByTestId('code-editor') as HTMLTextAreaElement;
    expect(editor.value).toContain('name: security-reviewer');
    fireEvent.change(editor, {
      target: {
        value: SAMPLE_RAW.replace('name: security-reviewer', 'name: raw-edited'),
      },
    });
    fireEvent.click(screen.getByRole('button', { name: 'Create agent' }));
    expect(onSubmit.mock.calls[0][0]).toContain('name: raw-edited');
  });

  it('renders the Identity/Model/Prompt card bodies by default', () => {
    render(
      <AgentBuilder
        initialRaw={SAMPLE_RAW}
        submitLabel="Create agent"
        submitting={false}
        error={null}
        onSubmit={vi.fn()}
      />,
    );
    expect(screen.getByLabelText('description')).toHaveValue('Structured security reviewer for panel workflows.');
    expect(screen.getByLabelText('system prompt')).toHaveValue(
      'You are a senior application-security reviewer.\n',
    );
    expect(screen.getByRole('button', { name: 'anthropic' })).toBeInTheDocument();
  });
});
