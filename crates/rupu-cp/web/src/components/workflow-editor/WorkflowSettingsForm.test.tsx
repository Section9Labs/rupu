// @vitest-environment jsdom
// WorkflowSettingsForm — flag-gated Trigger + Inputs authoring cards (Task 5).
// Model on StepForm.test.tsx's Harness pattern: a controlled wrapper re-renders
// the form with the latest emitted meta so mode switches / add-row flows
// re-render the dependent fields. Classic-path assertions confirm the
// read-only rest-chips form renders unchanged and the new cards are absent.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, fireEvent } from '@testing-library/react';
import { useState } from 'react';

import WorkflowSettingsForm from './WorkflowSettingsForm';
import type { WorkflowMeta } from '../../lib/workflowGraph';

function metaWith(rest: Record<string, unknown>): WorkflowMeta {
  return { name: 'wf', description: 'd', rest };
}

/** Controlled wrapper — re-renders the form with the latest emitted meta. */
function Harness({
  initial,
  spy,
  workflowEditorUi,
}: {
  initial: WorkflowMeta;
  spy: (m: WorkflowMeta) => void;
  workflowEditorUi?: 'classic' | 'next';
}) {
  const [meta, setMeta] = useState(initial);
  return (
    <WorkflowSettingsForm
      meta={meta}
      workflowEditorUi={workflowEditorUi}
      onChange={(m) => {
        spy(m);
        setMeta(m);
      }}
    />
  );
}

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe('WorkflowSettingsForm — classic (unchanged)', () => {
  it('renders the read-only rest-chips form and no Trigger/Inputs card', () => {
    render(<WorkflowSettingsForm meta={metaWith({ trigger: { on: 'cron', cron: '* * * * *' } })} onChange={() => {}} />);
    expect(screen.getByText('Preserved advanced keys — edit these in the YAML tab:')).toBeInTheDocument();
    expect(screen.getByText('trigger')).toBeInTheDocument();
    expect(screen.queryByTestId('trigger-card')).not.toBeInTheDocument();
    expect(screen.queryByTestId('inputs-card')).not.toBeInTheDocument();
  });

  it('defaults to classic when workflowEditorUi is omitted', () => {
    render(<WorkflowSettingsForm meta={metaWith({ inputs: { x: { type: 'string', required: false } } })} onChange={() => {}} />);
    expect(screen.queryByTestId('inputs-card')).not.toBeInTheDocument();
    expect(screen.getByText('inputs')).toBeInTheDocument();
  });

  it('editing the name emits a meta with rest preserved', () => {
    const spy = vi.fn();
    const meta = metaWith({ trigger: { cron: '* * * * *' } });
    render(<WorkflowSettingsForm meta={meta} onChange={spy} />);
    fireEvent.change(screen.getByLabelText('Workflow name'), { target: { value: 'new' } });
    expect(spy).toHaveBeenCalledWith(expect.objectContaining({ name: 'new', rest: { trigger: { cron: '* * * * *' } } }));
  });
});

describe('WorkflowSettingsForm — next: Trigger card', () => {
  it('renders a Trigger card and an Inputs card', () => {
    render(<WorkflowSettingsForm meta={metaWith({})} onChange={() => {}} workflowEditorUi="next" />);
    expect(screen.getByTestId('trigger-card')).toBeInTheDocument();
    expect(screen.getByTestId('inputs-card')).toBeInTheDocument();
    expect(screen.queryByText('Preserved advanced keys — edit these in the YAML tab:')).not.toBeInTheDocument();
  });

  it('defaults on = manual with no cron/event fields shown', () => {
    render(<WorkflowSettingsForm meta={metaWith({})} onChange={() => {}} workflowEditorUi="next" />);
    expect(screen.getByRole('button', { name: 'manual' })).toHaveAttribute('aria-pressed', 'true');
    expect(screen.queryByLabelText('Trigger cron')).not.toBeInTheDocument();
    expect(screen.queryByLabelText('Trigger event')).not.toBeInTheDocument();
  });

  it('selecting on=event shows event+filter fields and hides the cron field', () => {
    const spy = vi.fn();
    render(<Harness initial={metaWith({})} spy={spy} workflowEditorUi="next" />);

    fireEvent.click(screen.getByRole('button', { name: 'event' }));
    expect(screen.getByLabelText('Trigger event')).toBeInTheDocument();
    expect(screen.getByLabelText('Trigger filter')).toBeInTheDocument();
    expect(screen.queryByLabelText('Trigger cron')).not.toBeInTheDocument();
    expect(spy).toHaveBeenLastCalledWith(expect.objectContaining({ rest: { trigger: { on: 'event' } } }));
  });

  it('choosing on=cron and typing a cron emits rest.trigger = {on: cron, cron} with no event', () => {
    const spy = vi.fn();
    render(<Harness initial={metaWith({})} spy={spy} workflowEditorUi="next" />);

    fireEvent.click(screen.getByRole('button', { name: 'cron' }));
    fireEvent.change(screen.getByLabelText('Trigger cron'), { target: { value: '0 9 * * *' } });

    const last = spy.mock.calls[spy.mock.calls.length - 1][0] as WorkflowMeta;
    expect(last.rest.trigger).toEqual({ on: 'cron', cron: '0 9 * * *' });
    expect((last.rest.trigger as Record<string, unknown>).event).toBeUndefined();
  });

  it('reads an existing event trigger with filter back into the form', () => {
    render(
      <WorkflowSettingsForm
        meta={metaWith({ trigger: { on: 'event', event: 'github.pr.merged', filter: 'payload.merged' } })}
        onChange={() => {}}
        workflowEditorUi="next"
      />,
    );
    expect(screen.getByRole('button', { name: 'event' })).toHaveAttribute('aria-pressed', 'true');
    expect(screen.getByLabelText('Trigger event')).toHaveValue('github.pr.merged');
    expect(screen.getByLabelText('Trigger filter')).toHaveValue('payload.merged');
  });
});

describe('WorkflowSettingsForm — next: Inputs card', () => {
  it('adding an input named "target" with type "int" emits a name-keyed map with type', () => {
    const spy = vi.fn();
    render(<Harness initial={metaWith({})} spy={spy} workflowEditorUi="next" />);

    fireEvent.click(screen.getByRole('button', { name: '+ input' }));
    fireEvent.change(screen.getByLabelText('Input 1 name'), { target: { value: 'target' } });
    fireEvent.change(screen.getByLabelText('Input 1 type'), { target: { value: 'int' } });

    const last = spy.mock.calls[spy.mock.calls.length - 1][0] as WorkflowMeta;
    const inputs = last.rest.inputs as Record<string, { type: string }>;
    expect(inputs.target.type).toBe('int');
  });

  it('toggling required and setting a default round-trip through the emitted map', () => {
    const spy = vi.fn();
    render(<Harness initial={metaWith({})} spy={spy} workflowEditorUi="next" />);

    fireEvent.click(screen.getByRole('button', { name: '+ input' }));
    fireEvent.change(screen.getByLabelText('Input 1 name'), { target: { value: 'count' } });
    fireEvent.change(screen.getByLabelText('Input 1 type'), { target: { value: 'int' } });
    fireEvent.click(screen.getByLabelText('Input 1 required'));
    fireEvent.change(screen.getByLabelText('Input 1 default'), { target: { value: '3' } });

    const last = spy.mock.calls[spy.mock.calls.length - 1][0] as WorkflowMeta;
    const inputs = last.rest.inputs as Record<string, { type: string; required: boolean; default: unknown }>;
    expect(inputs.count).toEqual({ type: 'int', required: true, default: 3 });
  });

  it('adding an enum value renders a removable chip and emits it under enum', () => {
    const spy = vi.fn();
    render(<Harness initial={metaWith({})} spy={spy} workflowEditorUi="next" />);

    fireEvent.click(screen.getByRole('button', { name: '+ input' }));
    fireEvent.change(screen.getByLabelText('Input 1 name'), { target: { value: 'mode' } });
    fireEvent.change(screen.getByLabelText('Input 1 enum value'), { target: { value: 'fast' } });
    fireEvent.click(screen.getByRole('button', { name: 'Add enum value to input 1' }));

    expect(screen.getByText('fast')).toBeInTheDocument();
    let last = spy.mock.calls[spy.mock.calls.length - 1][0] as WorkflowMeta;
    let inputs = last.rest.inputs as Record<string, { enum?: string[] }>;
    expect(inputs.mode.enum).toEqual(['fast']);

    fireEvent.click(screen.getByRole('button', { name: 'Remove enum value fast from input 1' }));
    last = spy.mock.calls[spy.mock.calls.length - 1][0] as WorkflowMeta;
    inputs = last.rest.inputs as Record<string, { enum?: string[] }>;
    expect(inputs.mode.enum).toBeUndefined();
  });

  it('removing the only input omits the inputs key entirely', () => {
    const spy = vi.fn();
    render(<Harness initial={metaWith({ inputs: { x: { type: 'string', required: false } } })} spy={spy} workflowEditorUi="next" />);

    fireEvent.click(screen.getByRole('button', { name: 'Remove input 1' }));
    const last = spy.mock.calls[spy.mock.calls.length - 1][0] as WorkflowMeta;
    expect(last.rest.inputs).toBeUndefined();
  });

  it('reads an existing inputs map back into rows', () => {
    render(
      <WorkflowSettingsForm
        meta={metaWith({
          inputs: { repo: { type: 'string', required: true, description: 'target repo', enum: ['a', 'b'] } },
        })}
        onChange={() => {}}
        workflowEditorUi="next"
      />,
    );
    expect(screen.getByLabelText('Input 1 name')).toHaveValue('repo');
    expect(screen.getByLabelText('Input 1 type')).toHaveValue('string');
    expect(screen.getByLabelText('Input 1 required')).toBeChecked();
    expect(screen.getByLabelText('Input 1 description')).toHaveValue('target repo');
    expect(screen.getByText('a')).toBeInTheDocument();
    expect(screen.getByText('b')).toBeInTheDocument();
  });
});
