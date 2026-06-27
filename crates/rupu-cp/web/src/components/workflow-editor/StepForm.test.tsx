// @vitest-environment jsdom
// StepForm — the per-kind step editor. Drives the controlled inputs and asserts
// the emitted StepNodeData shape on each edit. A small Harness keeps `node` in
// state so that kind switches / gate toggles re-render the dependent fields.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, fireEvent } from '@testing-library/react';
import { useState } from 'react';
import StepForm from './StepForm';
import WorkflowSettingsForm from './WorkflowSettingsForm';
import type { GraphNode, StepNodeData, WorkflowMeta } from '../../lib/workflowGraph';
import type { AgentSummary } from '../../lib/api';

const AGENTS: AgentSummary[] = [
  { name: 'planner', usage: { tokens_in: 0, tokens_out: 0, tokens_cached: 0, cost_usd: 0 }, run_count: 0 },
  { name: 'coder', usage: { tokens_in: 0, tokens_out: 0, tokens_cached: 0, cost_usd: 0 }, run_count: 0 },
  { name: 'reviewer', usage: { tokens_in: 0, tokens_out: 0, tokens_cached: 0, cost_usd: 0 }, run_count: 0 },
] as unknown as AgentSummary[];

function nodeWith(data: Partial<StepNodeData>): GraphNode {
  return { id: data.id ?? 's1', data: { id: 's1', kind: 'step', ...data }, position: { x: 0, y: 0 } };
}

/** Controlled wrapper — re-renders StepForm with the latest emitted data. */
function Harness({ initial, spy }: { initial: GraphNode; spy: (d: StepNodeData) => void }) {
  const [node, setNode] = useState(initial);
  return (
    <StepForm
      node={node}
      agents={AGENTS}
      problems={[]}
      onChange={(d) => {
        spy(d);
        setNode((n) => ({ ...n, id: d.id, data: d }));
      }}
    />
  );
}

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe('StepForm', () => {
  it('editing the agent select on a linear step emits the new agent', () => {
    const spy = vi.fn();
    render(<StepForm node={nodeWith({ kind: 'step', agent: 'planner' })} agents={AGENTS} problems={[]} onChange={spy} />);
    fireEvent.change(screen.getByLabelText('Agent'), { target: { value: 'coder' } });
    expect(spy).toHaveBeenCalledWith(expect.objectContaining({ agent: 'coder', kind: 'step' }));
  });

  it('switching kind to parallel shows the sub-step editor and Add sub-step emits a 1-length array', () => {
    const spy = vi.fn();
    render(<Harness initial={nodeWith({ kind: 'step', agent: 'planner' })} spy={spy} />);

    fireEvent.change(screen.getByLabelText('Step kind'), { target: { value: 'parallel' } });
    expect(spy).toHaveBeenLastCalledWith(expect.objectContaining({ kind: 'parallel' }));

    fireEvent.click(screen.getByRole('button', { name: 'Add sub-step' }));
    const last = spy.mock.calls[spy.mock.calls.length - 1][0] as StepNodeData;
    expect(last.parallel).toHaveLength(1);
  });

  it('panel: toggling a panelist and enabling the gate emit under panel with exact keys', () => {
    const spy = vi.fn();
    render(<Harness initial={nodeWith({ kind: 'panel', panel: { panelists: [], subject: 'review' } })} spy={spy} />);

    fireEvent.click(screen.getByLabelText('Panelist reviewer'));
    let last = spy.mock.calls[spy.mock.calls.length - 1][0] as StepNodeData;
    expect(last.panel?.panelists).toEqual(['reviewer']);

    fireEvent.click(screen.getByLabelText('Enable gate'));
    fireEvent.change(screen.getByLabelText('Until no findings at severity or above'), {
      target: { value: 'medium' },
    });
    last = spy.mock.calls[spy.mock.calls.length - 1][0] as StepNodeData;
    expect(last.panel?.gate?.until_no_findings_at_severity_or_above).toBe('medium');
  });

  it('preserves raw_passthrough across an edit', () => {
    const spy = vi.fn();
    const pass = { contract: { foo: 'bar' } };
    render(
      <StepForm
        node={nodeWith({ kind: 'step', agent: 'planner', raw_passthrough: pass })}
        agents={AGENTS}
        problems={[]}
        onChange={spy}
      />,
    );
    fireEvent.change(screen.getByLabelText('Prompt'), { target: { value: 'do it' } });
    expect(spy).toHaveBeenCalledWith(expect.objectContaining({ raw_passthrough: pass }));
  });

  it('renders problems in an alert block', () => {
    render(
      <StepForm node={nodeWith({ kind: 'step' })} agents={AGENTS} problems={['needs an agent']} onChange={() => {}} />,
    );
    expect(screen.getByRole('alert')).toHaveTextContent('needs an agent');
  });
});

describe('WorkflowSettingsForm', () => {
  it('editing the name emits a meta with rest preserved', () => {
    const spy = vi.fn();
    const meta: WorkflowMeta = { name: 'old', description: 'd', rest: { trigger: { cron: '* * * * *' } } };
    render(<WorkflowSettingsForm meta={meta} onChange={spy} />);
    fireEvent.change(screen.getByLabelText('Workflow name'), { target: { value: 'new' } });
    expect(spy).toHaveBeenCalledWith(
      expect.objectContaining({ name: 'new', rest: { trigger: { cron: '* * * * *' } } }),
    );
  });
});
