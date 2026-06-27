// @vitest-environment jsdom
// WorkflowEditor — the debounced live YAML→graph reconcile (Phase 2).
//
// The heavy children are mocked: WorkflowEditorGraph surfaces the `paused` prop
// (and the node ids it received) into the DOM so we can assert on reconcile
// outcomes without mounting @xyflow/react; CodeEditor / SplitPane / the forms are
// thin stubs. Fake timers drive the 250ms debounce deterministically.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, act, fireEvent } from '@testing-library/react';
import type { WorkflowGraph } from '../../lib/workflowGraph';

vi.mock('./WorkflowEditorGraph', () => ({
  default: ({ graph, paused }: { graph: WorkflowGraph; paused?: boolean }) => (
    <div
      data-testid="graph"
      data-paused={paused ? 'true' : 'false'}
      data-ids={graph.nodes.map((n) => n.id).join(',')}
    />
  ),
}));

vi.mock('../CodeEditor', () => ({ default: () => <div data-testid="code" /> }));

vi.mock('./SplitPane', () => ({
  default: ({ top, bottom }: { top: React.ReactNode; bottom: React.ReactNode }) => (
    <div>
      {top}
      {bottom}
    </div>
  ),
}));

vi.mock('./StepForm', () => ({ default: () => <div /> }));
vi.mock('./WorkflowSettingsForm', () => ({ default: () => <div /> }));

import WorkflowEditor from './WorkflowEditor';

const VALID = 'name: wf\nsteps:\n  - id: a\n    agent: x\n    prompt: hi\n';

function renderEditor(draftYaml: string) {
  return render(
    <WorkflowEditor draftYaml={draftYaml} onYamlChange={() => {}} agents={[]} validity={null} />,
  );
}

describe('WorkflowEditor live reconcile', () => {
  afterEach(() => {
    cleanup();
    vi.useRealTimers();
  });

  it('pauses on invalid YAML after the debounce, keeping the existing nodes', () => {
    vi.useFakeTimers();
    const { rerender } = renderEditor(VALID);
    expect(screen.getByTestId('graph')).toHaveAttribute('data-paused', 'false');
    expect(screen.getByTestId('graph')).toHaveAttribute('data-ids', 'a');

    rerender(
      <WorkflowEditor draftYaml={'name: [oops\n :: bad'} onYamlChange={() => {}} agents={[]} validity={null} />,
    );
    act(() => {
      vi.advanceTimersByTime(250);
    });
    const g = screen.getByTestId('graph');
    expect(g).toHaveAttribute('data-paused', 'true');
    expect(g).toHaveAttribute('data-ids', 'a'); // graph kept, not nuked
  });

  it('reconciles valid YAML with a new node and clears paused', () => {
    vi.useFakeTimers();
    const { rerender } = renderEditor(VALID);

    const next = 'name: wf\nsteps:\n  - id: a\n    agent: x\n    prompt: hi\n  - id: b\n    agent: y\n    prompt: yo\n';
    rerender(<WorkflowEditor draftYaml={next} onYamlChange={() => {}} agents={[]} validity={null} />);
    act(() => {
      vi.advanceTimersByTime(250);
    });
    const g = screen.getByTestId('graph');
    expect(g).toHaveAttribute('data-paused', 'false');
    expect(g.getAttribute('data-ids')!.split(',').sort()).toEqual(['a', 'b']);
  });

  it('the Reference tab renders the expression reference panel', () => {
    renderEditor(VALID);
    fireEvent.click(screen.getByRole('tab', { name: 'Reference' }));
    const panel = screen.getByRole('tabpanel');
    expect(panel).toHaveAttribute('id', 'inspector-reference');
    expect(panel).toHaveAttribute('aria-labelledby', 'inspector-tab-reference');
    expect(screen.getByRole('searchbox', { name: 'Search expressions' })).toBeInTheDocument();
    expect(screen.getByRole('heading', { name: 'Inputs' })).toBeInTheDocument();
  });

  it('un-pauses once YAML parses again', () => {
    vi.useFakeTimers();
    const { rerender } = renderEditor(VALID);

    rerender(<WorkflowEditor draftYaml={'name: [oops'} onYamlChange={() => {}} agents={[]} validity={null} />);
    act(() => vi.advanceTimersByTime(250));
    expect(screen.getByTestId('graph')).toHaveAttribute('data-paused', 'true');

    rerender(<WorkflowEditor draftYaml={VALID} onYamlChange={() => {}} agents={[]} validity={null} />);
    act(() => vi.advanceTimersByTime(250));
    expect(screen.getByTestId('graph')).toHaveAttribute('data-paused', 'false');
  });
});
