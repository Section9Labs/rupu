// @vitest-environment jsdom
// WorkflowEditor — the debounced live YAML→graph reconcile (Phase 2).
//
// The heavy children are mocked: WorkflowEditorGraph surfaces the `paused` prop
// (and the node ids it received) into the DOM so we can assert on reconcile
// outcomes without mounting @xyflow/react; CodeEditor / SplitPane / the forms are
// thin stubs. Fake timers drive the 250ms debounce deterministically.

import '@testing-library/jest-dom/vitest';
import { afterEach, beforeEach, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, act, fireEvent } from '@testing-library/react';
import type { WorkflowGraph } from '../../lib/workflowGraph';

// `onSelect` is surfaced as a per-node "select-<id>" button so tests can drive
// selection (needed by the Task 6 convert-to-gate wiring test below) without
// mounting @xyflow/react.
vi.mock('./WorkflowEditorGraph', () => ({
  default: ({
    graph,
    paused,
    paletteContainer,
    onSelect,
  }: {
    graph: WorkflowGraph;
    paused?: boolean;
    paletteContainer?: HTMLElement | null;
    onSelect?: (id: string | null) => void;
  }) => (
    <div
      data-testid="graph"
      data-paused={paused ? 'true' : 'false'}
      data-ids={graph.nodes.map((n) => n.id).join(',')}
      data-palette-container={paletteContainer ? 'set' : 'none'}
    >
      {graph.nodes.map((n) => (
        <button key={n.id} onClick={() => onSelect?.(n.id)}>{`select-${n.id}`}</button>
      ))}
    </div>
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

// Surfaces the selected node id + `onConvertToGate` so the Task 6 wiring test
// below can drive the button without a real StepForm mount.
vi.mock('./StepForm', () => ({
  default: ({ node, onConvertToGate }: { node?: { id: string }; onConvertToGate?: () => void }) => (
    <div data-testid="stepform" data-node-id={node?.id ?? ''}>
      {onConvertToGate && <button onClick={onConvertToGate}>Convert to gate node</button>}
    </div>
  ),
}));
vi.mock('./WorkflowSettingsForm', () => ({ default: () => <div /> }));

import WorkflowEditor from './WorkflowEditor';

const VALID = 'name: wf\nsteps:\n  - id: a\n    agent: x\n    prompt: hi\n';

function renderEditor(draftYaml: string) {
  return render(
    <WorkflowEditor draftYaml={draftYaml} onYamlChange={() => {}} agents={[]} validity={null} />,
  );
}

// jsdom's localStorage is unreliable under this Node version — install a
// simple in-memory implementation we fully control (matches ThemeProvider.test.tsx).
// Shared by the Task 2 (source toggle) and Task 5 (rail width) describe blocks
// below, both of which persist to localStorage.
function installLocalStorage() {
  const store = new Map<string, string>();
  vi.stubGlobal('localStorage', {
    getItem: (k: string) => (store.has(k) ? store.get(k)! : null),
    setItem: (k: string, v: string) => store.set(k, String(v)),
    removeItem: (k: string) => store.delete(k),
    clear: () => store.clear(),
    key: (i: number) => Array.from(store.keys())[i] ?? null,
    get length() {
      return store.size;
    },
  });
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

  it('classic: no rail palette slot, no Blocks tab, and WorkflowEditorGraph gets no paletteContainer', () => {
    const { container } = render(
      <WorkflowEditor draftYaml={VALID} onYamlChange={() => {}} agents={[]} validity={null} />,
    );
    expect(container.querySelector('.wfx-rail-palette-slot')).not.toBeInTheDocument();
    expect(screen.queryByRole('tab', { name: 'Blocks' })).not.toBeInTheDocument();
    expect(screen.getByTestId('graph')).toHaveAttribute('data-palette-container', 'none');
    // classic default tab is still Settings.
    expect(screen.getByRole('tab', { name: 'Settings' })).toHaveAttribute('aria-selected', 'true');
  });

  it('next: renders 4 tabs (Blocks · Step · Settings · Reference) in that order, defaulting to Blocks', () => {
    render(
      <WorkflowEditor
        draftYaml={VALID}
        onYamlChange={() => {}}
        agents={[]}
        validity={null}
        workflowEditorUi="next"
      />,
    );
    const tabs = screen.getAllByRole('tab');
    expect(tabs.map((t) => t.textContent)).toEqual(['Blocks', 'Step', 'Settings', 'Reference']);
    expect(screen.getByRole('tab', { name: 'Blocks' })).toHaveAttribute('aria-selected', 'true');
  });

  it('next: the Blocks tab is the default and its tabpanel hosts the rail palette slot; WorkflowEditorGraph receives it', () => {
    const { container } = render(
      <WorkflowEditor
        draftYaml={VALID}
        onYamlChange={() => {}}
        agents={[]}
        validity={null}
        workflowEditorUi="next"
      />,
    );
    const panel = screen.getByRole('tabpanel');
    expect(panel).toHaveAttribute('id', 'inspector-blocks');
    const slot = container.querySelector('.wfx-rail-palette-slot');
    expect(slot).toBeInTheDocument();
    expect(panel.contains(slot)).toBe(true);
    expect(screen.getByTestId('graph')).toHaveAttribute('data-palette-container', 'set');
  });

  it('next: switching off the Blocks tab unmounts the palette slot (only mounted while Blocks is active)', () => {
    render(
      <WorkflowEditor
        draftYaml={VALID}
        onYamlChange={() => {}}
        agents={[]}
        validity={null}
        workflowEditorUi="next"
      />,
    );
    fireEvent.click(screen.getByRole('tab', { name: 'Step' }));
    expect(screen.queryByRole('tabpanel', { name: 'Blocks' })).not.toBeInTheDocument();
    expect(screen.getByTestId('graph')).toHaveAttribute('data-palette-container', 'none');
  });

  it('next: selecting a canvas node still auto-switches to the Step tab', () => {
    render(
      <WorkflowEditor
        draftYaml={VALID}
        onYamlChange={() => {}}
        agents={[]}
        validity={null}
        workflowEditorUi="next"
      />,
    );
    fireEvent.click(screen.getByRole('button', { name: 'select-a' }));
    expect(screen.getByRole('tab', { name: 'Step' })).toHaveAttribute('aria-selected', 'true');
    expect(screen.getByTestId('stepform')).toHaveAttribute('data-node-id', 'a');
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

describe('WorkflowEditor source pane toggle (Task 2)', () => {
  const SOURCE_OPEN_KEY = 'rupu.editor.sourceOpen';

  beforeEach(() => {
    installLocalStorage();
  });

  afterEach(() => {
    cleanup();
    vi.unstubAllGlobals();
  });

  it('classic: no source toggle button, YAML editor always present', () => {
    render(
      <WorkflowEditor draftYaml={VALID} onYamlChange={() => {}} agents={[]} validity={{ ok: true }} />,
    );
    expect(screen.queryByRole('button', { name: /source/i })).not.toBeInTheDocument();
    expect(screen.getByTestId('code')).toBeInTheDocument();
  });

  it('next: a "Hide source" toggle is present by default, editor mounted', () => {
    render(
      <WorkflowEditor
        draftYaml={VALID}
        onYamlChange={() => {}}
        agents={[]}
        validity={{ ok: true }}
        workflowEditorUi="next"
      />,
    );
    expect(screen.getByRole('button', { name: 'Hide source' })).toBeInTheDocument();
    expect(screen.getByTestId('code')).toBeInTheDocument();
  });

  it('next: clicking "Hide source" removes the YAML editor from the DOM, keeps the validity badge, flips the button label, and persists to localStorage; clicking again restores it', () => {
    render(
      <WorkflowEditor
        draftYaml={VALID}
        onYamlChange={() => {}}
        agents={[]}
        validity={{ ok: true }}
        workflowEditorUi="next"
      />,
    );
    expect(screen.getByText('✓ valid')).toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: 'Hide source' }));

    expect(screen.queryByTestId('code')).not.toBeInTheDocument();
    expect(screen.getByText('✓ valid')).toBeInTheDocument(); // badge stays visible
    expect(screen.getByRole('button', { name: 'Show source' })).toBeInTheDocument();
    expect(localStorage.getItem(SOURCE_OPEN_KEY)).toBe('0');

    fireEvent.click(screen.getByRole('button', { name: 'Show source' }));

    expect(screen.getByTestId('code')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Hide source' })).toBeInTheDocument();
    expect(localStorage.getItem(SOURCE_OPEN_KEY)).toBe('1');
  });

  it('next: initial state honors a pre-seeded "0" in localStorage (starts closed)', () => {
    localStorage.setItem(SOURCE_OPEN_KEY, '0');
    render(
      <WorkflowEditor
        draftYaml={VALID}
        onYamlChange={() => {}}
        agents={[]}
        validity={{ ok: true }}
        workflowEditorUi="next"
      />,
    );
    expect(screen.queryByTestId('code')).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Show source' })).toBeInTheDocument();
    expect(screen.getByText('✓ valid')).toBeInTheDocument();
  });

  it('the bottom bar child order is note, toggle button, badge in BOTH the open and closed states (Task 2 minor fix)', () => {
    render(
      <WorkflowEditor
        draftYaml={VALID}
        onYamlChange={() => {}}
        agents={[]}
        validity={{ ok: true }}
        workflowEditorUi="next"
      />,
    );

    // Open (source visible) state.
    let bar = screen.getByRole('button', { name: 'Hide source' }).parentElement!;
    expect(bar.children).toHaveLength(3);
    expect(bar.children[0].tagName).toBe('SPAN');
    expect(bar.children[0]).toHaveTextContent('synced from graph');
    expect(bar.children[1].tagName).toBe('BUTTON');
    expect(bar.children[1]).toHaveTextContent('Hide source');
    expect(bar.children[2].tagName).toBe('SPAN');
    expect(bar.children[2]).toHaveTextContent('✓ valid');

    fireEvent.click(screen.getByRole('button', { name: 'Hide source' }));

    // Closed (source hidden) state — same order.
    bar = screen.getByRole('button', { name: 'Show source' }).parentElement!;
    expect(bar.children).toHaveLength(3);
    expect(bar.children[0].tagName).toBe('SPAN');
    expect(bar.children[0]).toHaveTextContent('synced from graph');
    expect(bar.children[1].tagName).toBe('BUTTON');
    expect(bar.children[1]).toHaveTextContent('Show source');
    expect(bar.children[2].tagName).toBe('SPAN');
    expect(bar.children[2]).toHaveTextContent('✓ valid');
  });
});

describe('WorkflowEditor resizable inspector rail (Task 5, next only)', () => {
  const RAIL_WIDTH_KEY = 'rupu.editor.railWidth';

  beforeEach(() => {
    installLocalStorage();
  });

  afterEach(() => {
    cleanup();
    vi.unstubAllGlobals();
  });

  it('classic: no resize-inspector separator; aside keeps the literal lg:w-80 sizing', () => {
    const { container } = render(
      <WorkflowEditor draftYaml={VALID} onYamlChange={() => {}} agents={[]} validity={null} />,
    );
    const aside = container.querySelector('aside')!;
    expect(aside.className).toContain('lg:w-80');
    expect(screen.queryByRole('separator', { name: 'Resize inspector' })).not.toBeInTheDocument();
  });

  it('next: aside exposes a vertical "Resize inspector" separator and drops lg:w-80', () => {
    const { container } = render(
      <WorkflowEditor
        draftYaml={VALID}
        onYamlChange={() => {}}
        agents={[]}
        validity={null}
        workflowEditorUi="next"
      />,
    );
    const aside = container.querySelector('aside')!;
    expect(aside.className).not.toContain('lg:w-80');
    const sep = screen.getByRole('separator', { name: 'Resize inspector' });
    expect(sep).toHaveAttribute('aria-orientation', 'vertical');
    expect(sep).toHaveAttribute('tabindex', '0');
  });

  it('next: defaults to 320px when localStorage is empty', () => {
    render(
      <WorkflowEditor
        draftYaml={VALID}
        onYamlChange={() => {}}
        agents={[]}
        validity={null}
        workflowEditorUi="next"
      />,
    );
    const sep = screen.getByRole('separator', { name: 'Resize inspector' });
    const aside = sep.closest('aside')!;
    expect(aside.style.getPropertyValue('--wfx-rail-w')).toBe('320px');
    expect(sep).toHaveAttribute('aria-valuenow', '320');
  });

  it('next: ArrowLeft widens and ArrowRight narrows the rail by 16px, persisting each change', () => {
    render(
      <WorkflowEditor
        draftYaml={VALID}
        onYamlChange={() => {}}
        agents={[]}
        validity={null}
        workflowEditorUi="next"
      />,
    );
    const sep = screen.getByRole('separator', { name: 'Resize inspector' });
    const aside = sep.closest('aside')!;

    fireEvent.keyDown(sep, { key: 'ArrowLeft' });
    expect(aside.style.getPropertyValue('--wfx-rail-w')).toBe('336px');
    expect(localStorage.getItem(RAIL_WIDTH_KEY)).toBe('336');

    fireEvent.keyDown(sep, { key: 'ArrowRight' });
    fireEvent.keyDown(sep, { key: 'ArrowRight' });
    expect(aside.style.getPropertyValue('--wfx-rail-w')).toBe('304px');
    expect(localStorage.getItem(RAIL_WIDTH_KEY)).toBe('304');
  });

  it('next: clamps to [280, 640]', () => {
    render(
      <WorkflowEditor
        draftYaml={VALID}
        onYamlChange={() => {}}
        agents={[]}
        validity={null}
        workflowEditorUi="next"
      />,
    );
    const sep = screen.getByRole('separator', { name: 'Resize inspector' });
    const aside = sep.closest('aside')!;

    for (let i = 0; i < 30; i++) fireEvent.keyDown(sep, { key: 'ArrowRight' });
    expect(aside.style.getPropertyValue('--wfx-rail-w')).toBe('280px');

    for (let i = 0; i < 30; i++) fireEvent.keyDown(sep, { key: 'ArrowLeft' });
    expect(aside.style.getPropertyValue('--wfx-rail-w')).toBe('640px');
  });

  it('next: honors a pre-seeded width from localStorage', () => {
    localStorage.setItem(RAIL_WIDTH_KEY, '500');
    render(
      <WorkflowEditor
        draftYaml={VALID}
        onYamlChange={() => {}}
        agents={[]}
        validity={null}
        workflowEditorUi="next"
      />,
    );
    const sep = screen.getByRole('separator', { name: 'Resize inspector' });
    const aside = sep.closest('aside')!;
    expect(aside.style.getPropertyValue('--wfx-rail-w')).toBe('500px');
  });

  it('next: garbage localStorage falls back to the 320px default', () => {
    localStorage.setItem(RAIL_WIDTH_KEY, 'not-a-number');
    render(
      <WorkflowEditor
        draftYaml={VALID}
        onYamlChange={() => {}}
        agents={[]}
        validity={null}
        workflowEditorUi="next"
      />,
    );
    const sep = screen.getByRole('separator', { name: 'Resize inspector' });
    const aside = sep.closest('aside')!;
    expect(aside.style.getPropertyValue('--wfx-rail-w')).toBe('320px');
  });
});

describe('WorkflowEditor — Convert to gate node wiring (Task 6)', () => {
  afterEach(cleanup);

  it('selecting a legacy inline-approval step, then clicking Convert to gate node, emits YAML with a new gate step before it', () => {
    const yamlWithApproval =
      'name: wf\nsteps:\n' +
      '  - id: ship\n    agent: deployer\n    prompt: deploy it\n' +
      '    approval:\n      required: true\n      prompt: ok to ship?\n';
    const onYamlChange = vi.fn();
    render(
      <WorkflowEditor draftYaml={yamlWithApproval} onYamlChange={onYamlChange} agents={[]} validity={null} />,
    );

    fireEvent.click(screen.getByRole('button', { name: 'select-ship' }));
    expect(screen.getByTestId('stepform')).toHaveAttribute('data-node-id', 'ship');

    fireEvent.click(screen.getByRole('button', { name: 'Convert to gate node' }));

    expect(onYamlChange).toHaveBeenCalledTimes(1);
    const emitted = onYamlChange.mock.calls[0][0] as string;
    // a new gate step (id `ship-gate`) precedes `ship` in the emitted YAML, and
    // carries the moved-over prompt; `ship` no longer has `approval:`.
    const gateIdx = emitted.indexOf('ship-gate');
    const shipIdx = emitted.indexOf('id: ship\n');
    expect(gateIdx).toBeGreaterThan(-1);
    expect(shipIdx).toBeGreaterThan(-1);
    expect(gateIdx).toBeLessThan(shipIdx);
    expect(emitted).toContain('ok to ship?');
    expect(emitted.slice(shipIdx)).not.toContain('approval');
  });
});
