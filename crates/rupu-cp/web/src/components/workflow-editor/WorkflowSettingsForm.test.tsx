// @vitest-environment jsdom
// WorkflowSettingsForm — flag-gated Trigger + Inputs authoring cards (Task 5).
// Model on StepForm.test.tsx's Harness pattern: a controlled wrapper re-renders
// the form with the latest emitted meta so mode switches / add-row flows
// re-render the dependent fields. Classic-path assertions confirm the
// read-only rest-chips form renders unchanged and the new cards are absent.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, fireEvent, within } from '@testing-library/react';
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

  it('adding two rows before naming either keeps BOTH visible (blank-name collision regression)', () => {
    const spy = vi.fn();
    render(<Harness initial={metaWith({})} spy={spy} workflowEditorUi="next" />);

    fireEvent.click(screen.getByRole('button', { name: '+ input' }));
    fireEvent.click(screen.getByRole('button', { name: '+ input' }));

    // Both blank rows must still be present in the DOM — previously the
    // second row's blank name collided with the first in the name-keyed
    // map and one row silently vanished on re-render.
    expect(screen.getByLabelText('Input 1 name')).toBeInTheDocument();
    expect(screen.getByLabelText('Input 2 name')).toBeInTheDocument();

    fireEvent.change(screen.getByLabelText('Input 1 name'), { target: { value: 'alpha' } });
    fireEvent.change(screen.getByLabelText('Input 2 name'), { target: { value: 'beta' } });

    // Still both rows visible, now named distinctly.
    expect(screen.getByLabelText('Input 1 name')).toHaveValue('alpha');
    expect(screen.getByLabelText('Input 2 name')).toHaveValue('beta');

    const last = spy.mock.calls[spy.mock.calls.length - 1][0] as WorkflowMeta;
    const inputs = last.rest.inputs as Record<string, unknown>;
    expect(Object.keys(inputs).sort()).toEqual(['alpha', 'beta']);
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

describe('WorkflowSettingsForm — classic: autoflow stays a chip, not a card', () => {
  it('renders autoflow as a read-only chip and no autoflow card', () => {
    render(
      <WorkflowSettingsForm
        meta={metaWith({ autoflow: { enabled: true, entity: 'issue' } })}
        onChange={() => {}}
      />,
    );
    expect(screen.getByText('autoflow')).toBeInTheDocument();
    expect(screen.queryByTestId('autoflow-card')).not.toBeInTheDocument();
    expect(screen.queryByTestId('lifecycle-ribbon')).not.toBeInTheDocument();
  });
});

describe('WorkflowSettingsForm — next: Autoflow card + Lifecycle ribbon', () => {
  it('renders the Autoflow card (toggle only) and a disabled-hint lifecycle ribbon when absent', () => {
    render(<WorkflowSettingsForm meta={metaWith({})} onChange={() => {}} workflowEditorUi="next" />);
    expect(screen.getByTestId('autoflow-card')).toBeInTheDocument();
    expect(screen.getByRole('switch', { name: 'Autoflow enabled' })).toHaveAttribute('aria-checked', 'false');
    // Sections hidden while disabled.
    expect(screen.queryByRole('group', { name: 'Autoflow entity' })).not.toBeInTheDocument();
    expect(screen.getByTestId('lifecycle-ribbon')).toBeInTheDocument();
    expect(screen.getByText(/autoflow disabled/i)).toBeInTheDocument();
    // autoflow key not surfaced as a preserved-key chip (owned by the card).
    expect(screen.queryByText('autoflow')).not.toBeInTheDocument();
  });

  it('toggling autoflow on emits rest.autoflow.enabled === true and reveals sections', () => {
    const spy = vi.fn();
    render(<Harness initial={metaWith({})} spy={spy} workflowEditorUi="next" />);

    fireEvent.click(screen.getByRole('switch', { name: 'Autoflow enabled' }));

    const last = spy.mock.calls[spy.mock.calls.length - 1][0] as WorkflowMeta;
    expect((last.rest.autoflow as Record<string, unknown>).enabled).toBe(true);
    expect(screen.getByRole('group', { name: 'Autoflow entity' })).toBeInTheDocument();
  });

  it('entity=pull_request reveals draft+base; entity=issue hides them and clears any prior values', () => {
    const spy = vi.fn();
    render(
      <Harness
        initial={metaWith({ autoflow: { enabled: true, entity: 'issue', selector: {} } })}
        spy={spy}
        workflowEditorUi="next"
      />,
    );

    expect(screen.queryByLabelText('Autoflow draft filter')).not.toBeInTheDocument();
    expect(screen.queryByLabelText('Autoflow base branch')).not.toBeInTheDocument();

    const entityGroup = screen.getByRole('group', { name: 'Autoflow entity' });
    fireEvent.click(within(entityGroup).getByRole('button', { name: 'pull_request' }));
    expect(screen.getByLabelText('Autoflow base branch')).toBeInTheDocument();
    const draftGroup = screen.getByRole('group', { name: 'Autoflow draft filter' });
    fireEvent.click(within(draftGroup).getByRole('button', { name: 'only' }));
    fireEvent.change(screen.getByLabelText('Autoflow base branch'), { target: { value: 'main' } });

    let last = spy.mock.calls[spy.mock.calls.length - 1][0] as WorkflowMeta;
    let autoflow = last.rest.autoflow as { selector?: { draft?: string; base?: string } };
    expect(autoflow.selector?.draft).toBe('only');
    expect(autoflow.selector?.base).toBe('main');

    fireEvent.click(within(entityGroup).getByRole('button', { name: 'issue' }));
    expect(screen.queryByLabelText('Autoflow base branch')).not.toBeInTheDocument();

    last = spy.mock.calls[spy.mock.calls.length - 1][0] as WorkflowMeta;
    autoflow = last.rest.autoflow as { selector?: { draft?: string; base?: string } };
    expect(autoflow.selector?.draft).toBeUndefined();
    expect(autoflow.selector?.base).toBeUndefined();
  });

  it('setting reconcile_every="10m" and claim.ttl="3h" emits those exact duration strings', () => {
    const spy = vi.fn();
    render(
      <Harness
        initial={metaWith({ autoflow: { enabled: true, entity: 'issue', selector: {} } })}
        spy={spy}
        workflowEditorUi="next"
      />,
    );

    fireEvent.change(screen.getByLabelText('Autoflow reconcile every'), { target: { value: '10m' } });
    fireEvent.change(screen.getByLabelText('Autoflow claim ttl'), { target: { value: '3h' } });

    const last = spy.mock.calls[spy.mock.calls.length - 1][0] as WorkflowMeta;
    const autoflow = last.rest.autoflow as { reconcile_every: string; claim: { ttl: string } };
    expect(autoflow.reconcile_every).toBe('10m');
    expect(autoflow.claim.ttl).toBe('3h');
  });

  it('claim.key defaults to issue for entity=issue and pr_head_sha for entity=pull_request', () => {
    const spy = vi.fn();
    render(
      <Harness
        initial={metaWith({ autoflow: { enabled: true, entity: 'issue', selector: {} } })}
        spy={spy}
        workflowEditorUi="next"
      />,
    );
    expect(screen.getByRole('group', { name: 'Autoflow claim key' })).toHaveTextContent('issue');
    fireEvent.click(screen.getByRole('button', { name: 'pull_request' }));
    const claimGroup = screen.getByRole('group', { name: 'Autoflow claim key' });
    expect(within(claimGroup).getByRole('button', { name: 'pr_head_sha' })).toHaveAttribute('aria-pressed', 'true');
  });

  it('the outcome select lists contractOutputKeys and is disabled when there are none', () => {
    const first = render(
      <WorkflowSettingsForm
        meta={metaWith({ autoflow: { enabled: true, entity: 'issue', selector: {} } })}
        onChange={() => {}}
        workflowEditorUi="next"
      />,
    );
    expect(screen.getByLabelText('Autoflow outcome output')).toBeDisabled();
    first.unmount();

    render(
      <WorkflowSettingsForm
        meta={metaWith({
          autoflow: { enabled: true, entity: 'issue', selector: {} },
          contracts: { outputs: { pr_url: {}, summary: {} } },
        })}
        onChange={() => {}}
        workflowEditorUi="next"
      />,
    );
    const select = screen.getByLabelText('Autoflow outcome output') as HTMLSelectElement;
    expect(select).not.toBeDisabled();
    const optionValues = Array.from(select.options).map((o) => o.value);
    expect(optionValues).toEqual(expect.arrayContaining(['pr_url', 'summary']));

    fireEvent.change(select, { target: { value: 'pr_url' } });
  });

  it('authors_from and on_skip emit the right enums', () => {
    const spy = vi.fn();
    render(
      <Harness
        initial={metaWith({ autoflow: { enabled: true, entity: 'issue', selector: {} } })}
        spy={spy}
        workflowEditorUi="next"
      />,
    );

    const authorsFromGroup = screen.getByRole('group', { name: 'Autoflow authors from' });
    fireEvent.click(within(authorsFromGroup).getByRole('button', { name: 'org_members' }));
    const onSkipGroup = screen.getByRole('group', { name: 'Autoflow on skip' });
    fireEvent.click(within(onSkipGroup).getByRole('button', { name: 'label_needs_human' }));

    const last = spy.mock.calls[spy.mock.calls.length - 1][0] as WorkflowMeta;
    const selector = (last.rest.autoflow as { selector: { authors_from?: string; on_skip?: string } }).selector;
    expect(selector.authors_from).toBe('org_members');
    expect(selector.on_skip).toBe('label_needs_human');
  });

  it('adding a labels_all chip round-trips through the emitted selector', () => {
    const spy = vi.fn();
    render(
      <Harness
        initial={metaWith({ autoflow: { enabled: true, entity: 'issue', selector: {} } })}
        spy={spy}
        workflowEditorUi="next"
      />,
    );

    fireEvent.change(screen.getByLabelText('Autoflow labels_all value'), { target: { value: 'autoflow' } });
    fireEvent.click(screen.getByRole('button', { name: 'Add labels_all value' }));

    expect(screen.getByText('autoflow')).toBeInTheDocument();
    const last = spy.mock.calls[spy.mock.calls.length - 1][0] as WorkflowMeta;
    const selector = (last.rest.autoflow as { selector: { labels_all?: string[] } }).selector;
    expect(selector.labels_all).toEqual(['autoflow']);
  });

  it('an untouched load -> save round-trips rest.autoflow deep-equal', () => {
    const original = {
      enabled: true,
      entity: 'pull_request',
      selector: { states: ['open'], labels_all: ['autoflow'], limit: 50, draft: 'exclude', base: 'main' },
      wake_on: ['github.pr.updated'],
      reconcile_every: '15m',
      claim: { key: 'pr_head_sha', ttl: '2h' },
      workspace: { strategy: 'worktree', branch: 'autoflow/{{ id }}' },
      outcome: { output: 'summary' },
    };
    const spy = vi.fn();
    render(
      <Harness
        initial={metaWith({ autoflow: original, contracts: { outputs: { summary: {} } } })}
        spy={spy}
        workflowEditorUi="next"
      />,
    );
    // Touch something inert (name field) to force an emit without touching
    // any autoflow control.
    fireEvent.change(screen.getByLabelText('Workflow name'), { target: { value: 'wf-touched' } });
    const last = spy.mock.calls[spy.mock.calls.length - 1][0] as WorkflowMeta;
    expect(last.rest.autoflow).toEqual(original);
  });
});
