// @vitest-environment jsdom
// StepForm — the per-kind step editor. Drives the controlled inputs and asserts
// the emitted StepNodeData shape on each edit. A small Harness keeps `node` in
// state so that kind switches / gate toggles re-render the dependent fields.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, fireEvent } from '@testing-library/react';
import { useState } from 'react';

// Mock the CodeMirror-backed ExpressionField to a plain textarea so the form
// tests stay stable (CodeMirror in jsdom is brittle). The mock preserves the
// ariaLabel + value/onChange contract StepForm relies on, and surfaces the
// `size` prop as a `data-size` attribute so Task 5's size-threading tests can
// assert on it without depending on the real (CSS-driven) implementation.
vi.mock('./ExpressionField', () => ({
  default: ({
    value,
    onChange,
    ariaLabel,
    size,
  }: {
    value: string;
    onChange: (v: string) => void;
    ariaLabel?: string;
    size?: 'default' | 'large';
  }) => (
    <textarea
      aria-label={ariaLabel}
      value={value}
      onChange={(e) => onChange(e.target.value)}
      data-size={size ?? 'default'}
    />
  ),
}));

import StepForm from './StepForm';
import WorkflowSettingsForm from './WorkflowSettingsForm';
import type { GraphNode, StepNodeData, WorkflowMeta } from '../../lib/workflowGraph';
import type { ExprContext } from '../../lib/workflowExpressions';
import type { AgentSummary } from '../../lib/api';

const EXPR: Omit<ExprContext, 'isForEachPrompt' | 'isPanelField'> = {
  nodeKind: 'step',
  inputNames: [],
  priorSteps: [],
};

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
      exprContext={EXPR}
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
    render(
      <StepForm node={nodeWith({ kind: 'step', agent: 'planner' })} agents={AGENTS} problems={[]} exprContext={EXPR} onChange={spy} />,
    );
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
        exprContext={EXPR}
        onChange={spy}
      />,
    );
    fireEvent.change(screen.getByLabelText('Prompt'), { target: { value: 'do it' } });
    expect(spy).toHaveBeenCalledWith(expect.objectContaining({ raw_passthrough: pass }));
  });

  it('editing the prompt via the ExpressionField emits the new prompt', () => {
    const spy = vi.fn();
    render(<Harness initial={nodeWith({ kind: 'step', agent: 'planner' })} spy={spy} />);
    fireEvent.change(screen.getByLabelText('Prompt'), { target: { value: 'review {{ inputs.repo }}' } });
    expect(spy).toHaveBeenLastCalledWith(expect.objectContaining({ prompt: 'review {{ inputs.repo }}' }));
  });

  it('renders problems in an alert block', () => {
    render(
      <StepForm
        node={nodeWith({ kind: 'step' })}
        agents={AGENTS}
        problems={['needs an agent']}
        exprContext={EXPR}
        onChange={() => {}}
      />,
    );
    expect(screen.getByRole('alert')).toHaveTextContent('needs an agent');
  });
});

describe('StepForm — branch (flag-gated)', () => {
  it('the Kind select offers Branch (if) only when workflowEditorUi is next', () => {
    render(
      <StepForm
        node={nodeWith({ kind: 'step', agent: 'planner' })}
        agents={AGENTS}
        problems={[]}
        exprContext={EXPR}
        onChange={() => {}}
      />,
    );
    expect(screen.queryByRole('option', { name: 'Branch (if)' })).not.toBeInTheDocument();
  });

  it('the Kind select offers Branch (if) when workflowEditorUi is next', () => {
    render(
      <StepForm
        node={nodeWith({ kind: 'step', agent: 'planner' })}
        agents={AGENTS}
        problems={[]}
        exprContext={EXPR}
        onChange={() => {}}
        workflowEditorUi="next"
      />,
    );
    expect(screen.getByRole('option', { name: 'Branch (if)' })).toBeInTheDocument();
  });

  it('an existing branch node always offers Branch (if), even with the flag off', () => {
    render(
      <StepForm
        node={nodeWith({ kind: 'branch', condition: 'inputs.ok' })}
        agents={AGENTS}
        problems={[]}
        exprContext={EXPR}
        onChange={() => {}}
      />,
    );
    expect(screen.getByRole('option', { name: 'Branch (if)' })).toBeInTheDocument();
    expect(screen.getByLabelText('Step kind')).toHaveValue('branch');
  });

  it('selecting a branch node shows the condition field + then/else pickers, and edits flow to node data', () => {
    const spy = vi.fn();
    render(
      <StepForm
        node={nodeWith({ kind: 'branch', condition: '', thenTargets: [], elseTargets: [] })}
        agents={AGENTS}
        problems={[]}
        exprContext={EXPR}
        onChange={spy}
        allNodeIds={['s1', 'ok-path', 'fail-path']}
      />,
    );

    // Condition field.
    fireEvent.change(screen.getByLabelText('Branch condition'), {
      target: { value: 'inputs.score > 0.5' },
    });
    expect(spy).toHaveBeenLastCalledWith(
      expect.objectContaining({ kind: 'branch', condition: 'inputs.score > 0.5' }),
    );

    // Then/else pickers list every OTHER node id (not the branch's own id).
    expect(screen.getByLabelText('Then target ok-path')).toBeInTheDocument();
    expect(screen.getByLabelText('Else target fail-path')).toBeInTheDocument();
    expect(screen.queryByLabelText('Then target s1')).not.toBeInTheDocument();

    fireEvent.click(screen.getByLabelText('Then target ok-path'));
    expect(spy).toHaveBeenLastCalledWith(expect.objectContaining({ thenTargets: ['ok-path'] }));

    fireEvent.click(screen.getByLabelText('Else target fail-path'));
    expect(spy).toHaveBeenLastCalledWith(expect.objectContaining({ elseTargets: ['fail-path'] }));
  });

  it('branch hides the when/continue_on_error/approval block', () => {
    render(
      <StepForm
        node={nodeWith({ kind: 'branch', condition: 'x' })}
        agents={AGENTS}
        problems={[]}
        exprContext={EXPR}
        onChange={() => {}}
      />,
    );
    expect(screen.queryByLabelText('When condition')).not.toBeInTheDocument();
    expect(screen.queryByLabelText('Continue on error')).not.toBeInTheDocument();
    expect(screen.queryByLabelText('Require approval')).not.toBeInTheDocument();
  });
});

describe('StepForm — roomier long-text fields (Task 5, next only)', () => {
  it('classic: the Prompt field stays default-sized (no size prop threaded)', () => {
    render(
      <StepForm
        node={nodeWith({ kind: 'step', agent: 'planner' })}
        agents={AGENTS}
        problems={[]}
        exprContext={EXPR}
        onChange={() => {}}
      />,
    );
    expect(screen.getByLabelText('Prompt')).toHaveAttribute('data-size', 'default');
  });

  it('next: the Prompt field is sized large', () => {
    render(
      <StepForm
        node={nodeWith({ kind: 'step', agent: 'planner' })}
        agents={AGENTS}
        problems={[]}
        exprContext={EXPR}
        onChange={() => {}}
        workflowEditorUi="next"
      />,
    );
    expect(screen.getByLabelText('Prompt')).toHaveAttribute('data-size', 'large');
  });

  it('next: a parallel sub-step prompt is sized large', () => {
    render(
      <StepForm
        node={nodeWith({ kind: 'parallel', parallel: [{ id: 'sub-1', agent: 'planner', prompt: '' }] })}
        agents={AGENTS}
        problems={[]}
        exprContext={EXPR}
        onChange={() => {}}
        workflowEditorUi="next"
      />,
    );
    expect(screen.getByLabelText('Sub-step 1 prompt')).toHaveAttribute('data-size', 'large');
  });

  it('next: the panel Subject and Prompt fields are sized large; classic stays default', () => {
    const node = nodeWith({ kind: 'panel', panel: { panelists: [], subject: 'review' } });
    const { rerender } = render(
      <StepForm node={node} agents={AGENTS} problems={[]} exprContext={EXPR} onChange={() => {}} />,
    );
    expect(screen.getByLabelText('Panel subject')).toHaveAttribute('data-size', 'default');

    rerender(
      <StepForm
        node={node}
        agents={AGENTS}
        problems={[]}
        exprContext={EXPR}
        onChange={() => {}}
        workflowEditorUi="next"
      />,
    );
    expect(screen.getByLabelText('Panel subject')).toHaveAttribute('data-size', 'large');
    expect(screen.getByLabelText('Panel prompt')).toHaveAttribute('data-size', 'large');
  });
});

describe('StepForm — Approval prompt expression completions (Task 3, next only)', () => {
  function nodeWithApproval(): GraphNode {
    return nodeWith({ kind: 'step', agent: 'planner', approvalRequired: true, approvalPrompt: 'ok {{ inputs.x }}?' });
  }

  it('classic: Approval prompt stays a plain input (byte-identical)', () => {
    render(
      <StepForm node={nodeWithApproval()} agents={AGENTS} problems={[]} exprContext={EXPR} onChange={() => {}} />,
    );
    const field = screen.getByLabelText('Approval prompt');
    expect(field.tagName).toBe('INPUT');
    expect(field).toHaveValue('ok {{ inputs.x }}?');
  });

  it('next: Approval prompt renders the ExpressionField shell (mocked as a textarea)', () => {
    render(
      <StepForm
        node={nodeWithApproval()}
        agents={AGENTS}
        problems={[]}
        exprContext={EXPR}
        onChange={() => {}}
        workflowEditorUi="next"
      />,
    );
    const field = screen.getByLabelText('Approval prompt');
    expect(field.tagName).toBe('TEXTAREA');
    expect(field).toHaveValue('ok {{ inputs.x }}?');
  });

  it('next: editing the Approval prompt via ExpressionField emits the new value', () => {
    const spy = vi.fn();
    render(
      <StepForm
        node={nodeWithApproval()}
        agents={AGENTS}
        problems={[]}
        exprContext={EXPR}
        onChange={spy}
        workflowEditorUi="next"
      />,
    );
    fireEvent.change(screen.getByLabelText('Approval prompt'), { target: { value: 'confirm {{ inputs.y }}' } });
    expect(spy).toHaveBeenLastCalledWith(expect.objectContaining({ approvalPrompt: 'confirm {{ inputs.y }}' }));
  });
});

describe('StepForm — approval gate body (Task 5)', () => {
  it('a gate node shows the gate fields (prompt / auto approve / on timeout) and NOT the agent fields', () => {
    render(
      <StepForm
        node={nodeWith({ kind: 'approval_gate', approvalRequired: true })}
        agents={AGENTS}
        problems={[]}
        exprContext={EXPR}
        onChange={() => {}}
        workflowEditorUi="next"
      />,
    );
    expect(screen.getByLabelText('Approval prompt')).toBeInTheDocument();
    expect(screen.getByLabelText('Auto approve')).toBeInTheDocument();
    expect(screen.getByLabelText('On timeout')).toBeInTheDocument();
    // A gate is not an agent step — no Agent/Prompt fields, and the shared
    // inline "Require approval" checkbox is hidden (the gate owns approval).
    expect(screen.queryByLabelText('Agent')).not.toBeInTheDocument();
    expect(screen.queryByLabelText('Require approval')).not.toBeInTheDocument();
  });

  it('editing the auto-approve + on-timeout fields flows to node data', () => {
    const spy = vi.fn();
    render(
      <StepForm
        node={nodeWith({ kind: 'approval_gate', approvalRequired: true })}
        agents={AGENTS}
        problems={[]}
        exprContext={EXPR}
        onChange={spy}
        workflowEditorUi="next"
      />,
    );
    fireEvent.change(screen.getByLabelText('Auto approve'), { target: { value: '{{ inputs.ok }}' } });
    expect(spy).toHaveBeenLastCalledWith(expect.objectContaining({ approvalAutoApprove: '{{ inputs.ok }}' }));
    fireEvent.change(screen.getByLabelText('On timeout'), { target: { value: 'reject' } });
    expect(spy).toHaveBeenLastCalledWith(expect.objectContaining({ approvalOnTimeout: 'reject' }));
  });

  it('Add cleanup step appends an on-reject entry', () => {
    const spy = vi.fn();
    render(<Harness initial={nodeWith({ kind: 'approval_gate', approvalRequired: true, approvalOnReject: [] })} spy={spy} />);
    fireEvent.click(screen.getByRole('button', { name: 'Add cleanup step' }));
    const last = spy.mock.calls[spy.mock.calls.length - 1][0] as StepNodeData;
    expect(last.approvalOnReject).toHaveLength(1);
  });
});

describe('StepForm — Convert to gate node button (Task 6)', () => {
  it('renders when the step has an inline approval AND onConvertToGate is provided', () => {
    render(
      <StepForm
        node={nodeWith({ kind: 'step', agent: 'planner', approvalRequired: true })}
        agents={AGENTS}
        problems={[]}
        exprContext={EXPR}
        onChange={() => {}}
        onConvertToGate={() => {}}
      />,
    );
    expect(screen.getByRole('button', { name: 'Convert to gate node' })).toBeInTheDocument();
  });

  it('does not render without onConvertToGate (caller opted out)', () => {
    render(
      <StepForm
        node={nodeWith({ kind: 'step', agent: 'planner', approvalRequired: true })}
        agents={AGENTS}
        problems={[]}
        exprContext={EXPR}
        onChange={() => {}}
      />,
    );
    expect(screen.queryByRole('button', { name: 'Convert to gate node' })).not.toBeInTheDocument();
  });

  it('does not render when the step has no inline approval, even with onConvertToGate provided', () => {
    render(
      <StepForm
        node={nodeWith({ kind: 'step', agent: 'planner' })}
        agents={AGENTS}
        problems={[]}
        exprContext={EXPR}
        onChange={() => {}}
        onConvertToGate={() => {}}
      />,
    );
    expect(screen.queryByRole('button', { name: 'Convert to gate node' })).not.toBeInTheDocument();
  });

  it('does not render on a standalone approval_gate node (it IS the gate already)', () => {
    render(
      <StepForm
        node={nodeWith({ kind: 'approval_gate', approvalRequired: true })}
        agents={AGENTS}
        problems={[]}
        exprContext={EXPR}
        onChange={() => {}}
        onConvertToGate={() => {}}
      />,
    );
    expect(screen.queryByRole('button', { name: 'Convert to gate node' })).not.toBeInTheDocument();
  });

  it('clicking it invokes onConvertToGate', () => {
    const onConvertToGate = vi.fn();
    render(
      <StepForm
        node={nodeWith({ kind: 'step', agent: 'planner', approvalRequired: true })}
        agents={AGENTS}
        problems={[]}
        exprContext={EXPR}
        onChange={() => {}}
        onConvertToGate={onConvertToGate}
      />,
    );
    fireEvent.click(screen.getByRole('button', { name: 'Convert to gate node' }));
    expect(onConvertToGate).toHaveBeenCalledTimes(1);
  });
});

describe('StepForm — action body (Task 5)', () => {
  const TOOLS = [
    {
      name: 'scm.prs.create',
      description: 'Open a PR',
      input_schema: { properties: { title: {}, base: {} } },
      kind: 'write' as const,
    },
    { name: 'issues.comment', description: 'Comment', input_schema: { properties: { body: {} } }, kind: 'write' as const },
  ];

  function ActionHarness({ spy }: { spy: (d: StepNodeData) => void }) {
    const [node, setNode] = useState<GraphNode>(nodeWith({ kind: 'action', action: '', with: {} }));
    return (
      <StepForm
        node={node}
        agents={AGENTS}
        problems={[]}
        exprContext={EXPR}
        tools={TOOLS}
        onChange={(d) => {
          spy(d);
          setNode((n) => ({ ...n, id: d.id, data: d }));
        }}
      />
    );
  }

  it('shows a tool <select> populated from the catalog', () => {
    render(<ActionHarness spy={vi.fn()} />);
    const select = screen.getByLabelText('Action tool');
    expect(select).toBeInTheDocument();
    expect(screen.getByRole('option', { name: 'scm.prs.create' })).toBeInTheDocument();
    expect(screen.getByRole('option', { name: 'issues.comment' })).toBeInTheDocument();
  });

  it('selecting a tool renders a with-field per input_schema property', () => {
    const spy = vi.fn();
    render(<ActionHarness spy={spy} />);
    fireEvent.change(screen.getByLabelText('Action tool'), { target: { value: 'scm.prs.create' } });
    expect(spy).toHaveBeenLastCalledWith(expect.objectContaining({ action: 'scm.prs.create' }));
    expect(screen.getByLabelText('With title')).toBeInTheDocument();
    expect(screen.getByLabelText('With base')).toBeInTheDocument();
    fireEvent.change(screen.getByLabelText('With title'), { target: { value: 'Fix bug' } });
    const last = spy.mock.calls[spy.mock.calls.length - 1][0] as StepNodeData;
    expect(last.with).toEqual({ title: 'Fix bug' });
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

  it('classic: the description field keeps rows=3 (byte-identical)', () => {
    const meta: WorkflowMeta = { name: 'wf', description: '', rest: {} };
    render(<WorkflowSettingsForm meta={meta} onChange={() => {}} />);
    expect(screen.getByLabelText('Workflow description')).toHaveAttribute('rows', '3');
  });

  it('next: the description field is roomier (rows=4)', () => {
    const meta: WorkflowMeta = { name: 'wf', description: '', rest: {} };
    render(<WorkflowSettingsForm meta={meta} onChange={() => {}} workflowEditorUi="next" />);
    expect(screen.getByLabelText('Workflow description')).toHaveAttribute('rows', '4');
  });
});
