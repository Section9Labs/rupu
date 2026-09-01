// @vitest-environment jsdom
/**
 * Tests for Turn:
 *   - collapsed (defaultOpen=false): summary pills + snippet visible, but the
 *     full ToolCard body / markdown is NOT in the DOM.
 *   - clicking the header expands and reveals a ToolCard's content.
 *   - findingCount > 0 renders the findings pill.
 *   - result pill reflects summary.result.
 */

import { it, expect, describe, afterEach } from 'vitest';
import { render, screen, cleanup, fireEvent } from '@testing-library/react';
import Turn from './Turn';
import type { TurnBlock, TurnView, ToolView } from './transcriptView';

afterEach(cleanup);

function readTool(overrides?: Partial<ToolView>): ToolView {
  return {
    tool: 'read_file',
    kind: 'read',
    input: { path: 'src/lib/api.ts' },
    output: 'UNIQUE_TOOL_OUTPUT_MARKER',
    durationMs: 12,
    ...overrides,
  };
}

// v2 blocks model: TurnView no longer carries `assistant`/`tools` fields
// directly — build the equivalent `blocks` array instead. Mechanical
// adaptation only (Task 3 owns the real Turn.tsx blocks rendering).
function makeTurn(overrides?: { tools?: ToolView[]; summary?: TurnView['summary'] }): TurnView {
  const tools = overrides?.tools ?? [readTool()];
  const findingCount = tools.filter((t) => t.kind === 'finding').length;
  const blocks: TurnBlock[] = [
    { kind: 'assistant', content: 'Looking at the API surface now.' },
    ...tools.map((view): TurnBlock => ({ kind: 'tool', view })),
  ];
  return {
    blocks,
    tokensIn: null,
    tokensOut: null,
    summary: overrides?.summary ?? {
      toolCount: tools.length,
      findingCount,
      result: 'ok',
    },
  };
}

describe('Turn', () => {
  it('collapsed: shows snippet + pills but not the ToolCard body', () => {
    render(<Turn turn={makeTurn()} defaultOpen={false} />);

    // Snippet visible
    expect(screen.getByText(/Looking at the API surface now\./)).toBeTruthy();
    // Tool-count pill
    expect(screen.getByText(/1 tool/)).toBeTruthy();
    // Result pill
    expect(screen.getByText('ok')).toBeTruthy();
    // ToolCard body content is NOT rendered while collapsed
    expect(screen.queryByText('UNIQUE_TOOL_OUTPUT_MARKER')).toBeNull();
  });

  it('expands on header click to reveal the ToolCard content', () => {
    render(<Turn turn={makeTurn()} defaultOpen={false} />);

    expect(screen.queryByText('UNIQUE_TOOL_OUTPUT_MARKER')).toBeNull();

    // The header is the first button.
    fireEvent.click(screen.getAllByRole('button')[0]);

    expect(screen.getByText('UNIQUE_TOOL_OUTPUT_MARKER')).toBeTruthy();
  });

  it('renders the findings pill when findingCount > 0', () => {
    const turn = makeTurn({
      tools: [
        {
          tool: 'report_finding',
          kind: 'finding',
          input: { summary: 'a problem' },
          finding: {
            severity: 'medium',
            summary: 'a problem',
            scope: '',
            rationale: 'because',
            references: [],
          },
        },
      ],
      summary: { toolCount: 1, findingCount: 1, result: 'ok' },
    });
    render(<Turn turn={turn} defaultOpen={false} />);
    expect(screen.getByText(/1 finding/)).toBeTruthy();
  });

  it('result pill reflects summary.result (error)', () => {
    const turn = makeTurn({
      summary: { toolCount: 1, findingCount: 0, result: 'error' },
    });
    render(<Turn turn={turn} defaultOpen={false} />);
    expect(screen.getByText('error')).toBeTruthy();
  });

  it('result pill reflects summary.result (running)', () => {
    const turn = makeTurn({
      summary: { toolCount: 1, findingCount: 0, result: 'running' },
    });
    render(<Turn turn={turn} defaultOpen={false} />);
    expect(screen.getByText('running')).toBeTruthy();
  });

  it('defaultOpen=true renders the body immediately', () => {
    render(<Turn turn={makeTurn()} defaultOpen={true} />);
    expect(screen.getByText('UNIQUE_TOOL_OUTPUT_MARKER')).toBeTruthy();
  });
});
