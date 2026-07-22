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

  it('renders the Identity/Prompt card bodies by default, and Model when provider/model are set', () => {
    // Model is not always-wanted (see "removing the Model card sticks"
    // below) — it only surfaces when its owned fields have a value, same as
    // every other non-required card. Seed `provider` here so this test can
    // still assert its body renders.
    const RAW_WITH_MODEL = SAMPLE_RAW.replace('permissionMode: readonly', 'permissionMode: readonly\nprovider: anthropic');
    render(
      <AgentBuilder
        initialRaw={RAW_WITH_MODEL}
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

  it('adding a tool from the Tools card palette suggestion updates the preview', () => {
    render(
      <AgentBuilder
        initialRaw={SAMPLE_RAW}
        submitLabel="Create agent"
        submitting={false}
        error={null}
        onSubmit={vi.fn()}
      />,
    );
    fireEvent.click(screen.getByLabelText('add Tools card'));
    fireEvent.click(screen.getByText('+ bash'));
    expect(screen.getByTestId('ab-yaml')).toHaveTextContent(/tools:/);
    expect(screen.getByTestId('ab-yaml')).toHaveTextContent('bash');
  });

  it('toggling the Permission card to bypass updates the preview', () => {
    render(
      <AgentBuilder
        initialRaw={SAMPLE_RAW}
        submitLabel="Create agent"
        submitting={false}
        error={null}
        onSubmit={vi.fn()}
      />,
    );
    // SAMPLE_RAW sets permissionMode: readonly, so the Permission card is
    // already on the canvas at mount.
    fireEvent.click(screen.getByRole('button', { name: 'bypass' }));
    expect(screen.getByTestId('ab-yaml')).toHaveTextContent('permissionMode: bypass');
  });

  it('switching Output to json and adding a schema property updates the preview', () => {
    render(
      <AgentBuilder
        initialRaw={SAMPLE_RAW}
        submitLabel="Create agent"
        submitting={false}
        error={null}
        onSubmit={vi.fn()}
      />,
    );
    fireEvent.click(screen.getByLabelText('add Output card'));
    fireEvent.click(screen.getByRole('button', { name: 'json' }));
    fireEvent.click(screen.getByRole('button', { name: '+ property' }));
    fireEvent.change(screen.getByLabelText('schema property 0 name'), { target: { value: 'severity' } });
    expect(screen.getByTestId('ab-yaml')).toHaveTextContent(/outputSchema:/);
    expect(screen.getByTestId('ab-yaml')).toHaveTextContent('severity');
  });

  it('adding an inline concern updates the preview with its id and name', () => {
    render(
      <AgentBuilder
        initialRaw={SAMPLE_RAW}
        submitLabel="Create agent"
        submitting={false}
        error={null}
        onSubmit={vi.fn()}
      />,
    );
    fireEvent.click(screen.getByLabelText('add Concerns card'));
    fireEvent.click(screen.getByRole('button', { name: '+ inline concern' }));
    fireEvent.change(screen.getByLabelText('concern 0 id'), { target: { value: 'sqli' } });
    fireEvent.change(screen.getByLabelText('concern 0 name'), { target: { value: 'SQL Injection' } });
    const yaml = screen.getByTestId('ab-yaml');
    expect(yaml).toHaveTextContent(/id:/);
    expect(yaml).toHaveTextContent('sqli');
    expect(yaml).toHaveTextContent(/name:/);
    expect(yaml).toHaveTextContent('SQL Injection');
  });

  it('threads agentNames into the Dispatch card as chip suggestions', () => {
    render(
      <AgentBuilder
        initialRaw={SAMPLE_RAW}
        submitLabel="Create agent"
        submitting={false}
        error={null}
        onSubmit={vi.fn()}
        agentNames={['code-reviewer', 'pr-author']}
      />,
    );
    fireEvent.click(screen.getByLabelText('add Dispatch card'));
    expect(screen.getByText('+ code-reviewer')).toBeInTheDocument();
    fireEvent.click(screen.getByText('+ code-reviewer'));
    expect(screen.getByTestId('ab-yaml')).toHaveTextContent('code-reviewer');
  });

  it('shows an empty-body warning without disabling submit', () => {
    render(
      <AgentBuilder
        initialRaw={SAMPLE_RAW}
        submitLabel="Create agent"
        submitting={false}
        error={null}
        onSubmit={vi.fn()}
      />,
    );
    fireEvent.change(screen.getByLabelText('system prompt'), { target: { value: '' } });
    expect(screen.getByRole('button', { name: 'Create agent' })).not.toBeDisabled();
  });

  it('surfaces a card for fields set via Raw mode when switching back to Cards', () => {
    render(
      <AgentBuilder
        initialRaw={SAMPLE_RAW}
        submitLabel="Create agent"
        submitting={false}
        error={null}
        onSubmit={vi.fn()}
      />,
    );
    fireEvent.click(screen.getByRole('button', { name: 'Raw' }));
    const editor = screen.getByTestId('code-editor') as HTMLTextAreaElement;
    fireEvent.change(editor, {
      target: { value: editor.value.replace('permissionMode: readonly', 'permissionMode: readonly\ntools:\n  - bash') },
    });
    fireEvent.click(screen.getByRole('button', { name: 'Cards' }));
    expect(screen.getByRole('button', { name: 'remove Tools card' })).toBeInTheDocument();
  });

  it('supports real HTML5 drag-and-drop from the palette onto the canvas', () => {
    render(
      <AgentBuilder
        initialRaw={SAMPLE_RAW}
        submitLabel="Create agent"
        submitting={false}
        error={null}
        onSubmit={vi.fn()}
      />,
    );
    const store = new Map<string, string>();
    const dataTransfer = {
      setData: (k: string, v: string) => store.set(k, v),
      getData: (k: string) => store.get(k) ?? '',
      get types() {
        return Array.from(store.keys());
      },
      effectAllowed: '',
      dropEffect: '',
    };
    const source = screen.getByLabelText('add Reasoning card');
    fireEvent.dragStart(source, { dataTransfer });
    fireEvent.drop(screen.getByTestId('ab-canvas-drop'), { dataTransfer });
    expect(screen.getByRole('button', { name: 'remove Reasoning card' })).toBeInTheDocument();
  });

  it('removing the Model card sticks — no snap-back, config cleared from the preview', () => {
    const RAW_WITH_MODEL = SAMPLE_RAW.replace(
      'permissionMode: readonly',
      'permissionMode: readonly\nprovider: anthropic\nmodel: claude-sonnet-4-6',
    );
    render(
      <AgentBuilder
        initialRaw={RAW_WITH_MODEL}
        submitLabel="Create agent"
        submitting={false}
        error={null}
        onSubmit={vi.fn()}
      />,
    );
    // Model card is present at mount because provider/model are set.
    expect(screen.getByRole('button', { name: 'remove Model card' })).toBeInTheDocument();
    expect(screen.getByTestId('ab-yaml')).toHaveTextContent(/provider:/);
    expect(screen.getByTestId('ab-yaml')).toHaveTextContent(/model:/);

    fireEvent.click(screen.getByRole('button', { name: 'remove Model card' }));

    // Card is gone and the fields are cleared from the live preview...
    expect(screen.queryByRole('button', { name: 'remove Model card' })).not.toBeInTheDocument();
    expect(screen.getByTestId('ab-yaml')).not.toHaveTextContent('provider:');
    expect(screen.getByTestId('ab-yaml')).not.toHaveTextContent('model:');

    // ...and it does not snap back on a later render (the reactive
    // card-order effect must not re-add Model just because it's "always
    // wanted" — it isn't, anymore).
    expect(screen.queryByRole('button', { name: 'remove Model card' })).not.toBeInTheDocument();
    expect(screen.getByTestId('ab-yaml')).not.toHaveTextContent(/provider:/);
  });

  it('AI tab: generates via onGenerate with the selected provider/model and repopulates cards', async () => {
    const onGenerate = vi.fn().mockResolvedValue({
      raw: '---\nname: gen-agent\nprovider: anthropic\n---\n\nbody',
      provider: 'anthropic',
      model: 'x',
      attempts: 1,
    });
    render(
      <AgentBuilder
        initialRaw={SAMPLE_RAW}
        submitLabel="Create agent"
        submitting={false}
        error={null}
        onSubmit={vi.fn()}
        onGenerate={onGenerate}
        aiModels={[{ provider: 'anthropic', models: ['claude-sonnet-4-6'], is_default: true }]}
      />,
    );

    fireEvent.click(screen.getByRole('button', { name: 'AI' }));
    fireEvent.change(screen.getByLabelText('generation provider'), { target: { value: 'anthropic' } });
    fireEvent.change(screen.getByLabelText('describe the agent'), {
      target: { value: 'a read-only security reviewer' },
    });
    fireEvent.click(screen.getByRole('button', { name: /generate/i }));

    await screen.findByLabelText('agent name');
    expect(onGenerate).toHaveBeenCalledWith({
      description: 'a read-only security reviewer',
      provider: 'anthropic',
      model: 'claude-sonnet-4-6',
    });
    expect(screen.getByLabelText('agent name')).toHaveValue('gen-agent');
    // Mode returned to Cards — the AI description bar is gone.
    expect(screen.queryByLabelText('describe the agent')).not.toBeInTheDocument();
  });

  it('AI tab: shows the generate error when onGenerate rejects', async () => {
    const onGenerate = vi.fn().mockRejectedValue(new Error('boom generate'));
    render(
      <AgentBuilder
        initialRaw={SAMPLE_RAW}
        submitLabel="Create agent"
        submitting={false}
        error={null}
        onSubmit={vi.fn()}
        onGenerate={onGenerate}
        aiModels={[{ provider: 'anthropic', models: ['claude-sonnet-4-6'], is_default: true }]}
      />,
    );
    fireEvent.click(screen.getByRole('button', { name: 'AI' }));
    fireEvent.change(screen.getByLabelText('describe the agent'), { target: { value: 'a thing' } });
    fireEvent.click(screen.getByRole('button', { name: /generate/i }));
    expect(await screen.findByText('boom generate')).toBeInTheDocument();
  });

  it('Raw round-trip: editing Raw and switching to Cards reflects the change, and card edits show in Raw', () => {
    render(
      <AgentBuilder
        initialRaw={SAMPLE_RAW}
        submitLabel="Create agent"
        submitting={false}
        error={null}
        onSubmit={vi.fn()}
      />,
    );
    // Raw -> Cards.
    fireEvent.click(screen.getByRole('button', { name: 'Raw' }));
    const editor = screen.getByTestId('code-editor') as HTMLTextAreaElement;
    fireEvent.change(editor, {
      target: { value: SAMPLE_RAW.replace('name: security-reviewer', 'name: from-raw') },
    });
    fireEvent.click(screen.getByRole('button', { name: 'Cards' }));
    expect(screen.getByLabelText('agent name')).toHaveValue('from-raw');

    // Cards -> Raw.
    fireEvent.change(screen.getByLabelText('agent name'), { target: { value: 'from-card' } });
    fireEvent.click(screen.getByRole('button', { name: 'Raw' }));
    expect((screen.getByTestId('code-editor') as HTMLTextAreaElement).value).toContain('name: from-card');
  });

  it('dragging a card header before another card reorders the canvas', () => {
    render(
      <AgentBuilder
        initialRaw={SAMPLE_RAW}
        submitLabel="Create agent"
        submitting={false}
        error={null}
        onSubmit={vi.fn()}
      />,
    );
    // SAMPLE_RAW seeds Identity, Model (no, not anymore — see above test),
    // Permission (permissionMode: readonly) and Prompt at mount. Add
    // Reasoning so we have a well-separated pair of cards to reorder:
    // canvas order becomes [identity, permission, prompt, reasoning].
    fireEvent.click(screen.getByLabelText('add Reasoning card'));

    function cardOrder(): string[] {
      return Array.from(document.querySelectorAll('.ab-card .ab-ct')).map((el) => el.textContent ?? '');
    }

    const before = cardOrder();
    expect(before).toEqual(['Identity', 'Permission', 'Prompt', 'Reasoning']);

    const store = new Map<string, string>();
    const dataTransfer = {
      setData: (k: string, v: string) => store.set(k, v),
      getData: (k: string) => store.get(k) ?? '',
      get types() {
        return Array.from(store.keys());
      },
      effectAllowed: '',
      dropEffect: '',
    };

    // Drag the Reasoning header onto the Identity card — moveCardBefore
    // should splice Reasoning to sit immediately before Identity. Scope to
    // `.ab-ct` (the canvas card title span) since "Reasoning"/"Identity"
    // also appear as palette entries.
    const reasoningHeader = screen
      .getByText('Reasoning', { selector: '.ab-ct' })
      .closest('.ab-card-head') as HTMLElement;
    const identityCard = screen.getByText('Identity', { selector: '.ab-ct' }).closest('.ab-card') as HTMLElement;
    fireEvent.dragStart(reasoningHeader, { dataTransfer });
    fireEvent.dragOver(identityCard, { dataTransfer });
    fireEvent.drop(identityCard, { dataTransfer });

    const after = cardOrder();
    expect(after).toEqual(['Reasoning', 'Identity', 'Permission', 'Prompt']);
    expect(after.indexOf('Reasoning')).toBeLessThan(after.indexOf('Identity'));
    expect(after).not.toEqual(before);
  });
});
