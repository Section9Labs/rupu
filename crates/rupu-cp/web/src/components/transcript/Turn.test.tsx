// @vitest-environment jsdom
/**
 * Tests for Turn:
 *   - collapsed (defaultOpen=false): summary pills + snippet visible, but the
 *     full ToolCard body / markdown is NOT in the DOM.
 *   - clicking the header expands and reveals a ToolCard's content.
 *   - findingCount > 0 renders the findings pill.
 *   - result pill reflects summary.result.
 */

import '@testing-library/jest-dom/vitest';
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

  it('renders blocks in order: thinking (collapsible, full text), assistant, gate', () => {
    render(
      <Turn
        defaultOpen
        turn={{
          blocks: [
            { kind: 'thinking', text: 'long reasoning body', provider: 'anthropic' },
            { kind: 'assistant', content: 'the answer' },
            { kind: 'gate', gateId: 'g1', prompt: 'ship it?', decision: null, decidedBy: null },
          ],
          tokensIn: 11,
          tokensOut: 7,
          summary: { toolCount: 0, findingCount: 0, result: 'ok' },
        }}
      />,
    );
    fireEvent.click(screen.getByText('thinking'));
    expect(screen.getByText('long reasoning body')).toBeInTheDocument();
    // "the answer" legitimately appears twice with defaultOpen: once in the
    // header snippet, once in the body's rendered assistant block.
    expect(screen.getAllByText('the answer').length).toBeGreaterThan(0);
    expect(screen.getByText('ship it?')).toBeInTheDocument();
    expect(screen.getByText(/11.*→.*7|11 in.*7 out/)).toBeInTheDocument();
  });

  it('renders a redacted thinking block as a marker, and notice/seed/compaction/unknown rows', () => {
    render(
      <Turn
        defaultOpen
        turn={{
          blocks: [
            { kind: 'seed', messageCount: 4, sourceTranscript: null },
            { kind: 'user', content: 'do the task' },
            { kind: 'thinking', text: null, provider: 'anthropic' },
            { kind: 'notice', noticeKind: 'context_trim', message: 'trimmed to fit' },
            { kind: 'compaction', seq: 1, summarized: 9 },
            { kind: 'unknown', type: 'hologram_projection' },
          ],
          tokensIn: null,
          tokensOut: null,
          summary: { toolCount: 0, findingCount: 0, result: 'running' },
        }}
      />,
    );
    expect(screen.getByText(/redacted reasoning/)).toBeInTheDocument();
    expect(screen.getByText(/4 prior messages/)).toBeInTheDocument();
    expect(screen.getByText('do the task')).toBeInTheDocument();
    expect(screen.getByText(/trimmed to fit/)).toBeInTheDocument();
    expect(screen.getByText(/summarized 9/)).toBeInTheDocument();
    expect(screen.getByText(/hologram_projection/)).toBeInTheDocument();
  });
});
