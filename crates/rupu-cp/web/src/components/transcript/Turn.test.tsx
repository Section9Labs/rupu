// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { it, expect, describe, afterEach } from 'vitest';
import { render, screen, cleanup, fireEvent } from '@testing-library/react';
import Turn from './Turn';
import { TranscriptViewProvider } from './TranscriptViewContext';
import type { TurnBlock, TurnView, ToolView } from './transcriptView';

afterEach(cleanup);

function readTool(): ToolView {
  return { tool: 'read_file', kind: 'read', input: { path: 'src/lib/api.ts' }, output: 'OUT', durationMs: 12 };
}

function turnOf(blocks: TurnBlock[]): TurnView {
  return { blocks, tokensIn: null, tokensOut: null, summary: { toolCount: 0, findingCount: 0, result: 'ok' } };
}

const basic = turnOf([
  { kind: 'assistant', content: 'Looking at the API surface now.' },
  { kind: 'tool', view: readTool() },
]);

describe('Turn', () => {
  it('renders assistant text, a tool row and the marker', () => {
    render(<Turn turn={basic} turnNumber={1} showMarker />);
    expect(screen.getByText(/Looking at the API surface now\./)).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /read_file/ })).toBeInTheDocument();
    expect(screen.getByText('Turn 1')).toBeInTheDocument();
    expect(screen.getAllByText('1').length).toBeGreaterThan(0);
  });

  it('showMarker=false renders no turn marker', () => {
    render(<Turn turn={basic} turnNumber={3} showMarker={false} />);
    expect(screen.queryByText('Turn 3')).toBeNull();
    expect(screen.queryByText('3')).toBeNull();
    expect(screen.getByText(/Looking at the API/)).toBeInTheDocument();
  });

  it('shows the token label', () => {
    render(<Turn turn={basic} turnNumber={1} showMarker tokensLabel="11 in · 7 out" />);
    expect(screen.getByText('11 in · 7 out')).toBeInTheDocument();
  });

  it('thinking is shown in full and individually collapsible', () => {
    render(<Turn turn={turnOf([{ kind: 'thinking', text: 'long reasoning body', provider: 'anthropic' }])} turnNumber={1} showMarker />);
    expect(screen.getByText('long reasoning body')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: /thinking/ }));
    expect(screen.queryByText('long reasoning body')).toBeNull();
  });

  it('thinking folds when collapse-all is active', () => {
    render(
      <TranscriptViewProvider value={{ filter: 'all', expandTarget: 'closed', expandNonce: 1, live: false }}>
        <Turn turn={turnOf([{ kind: 'thinking', text: 'long reasoning body', provider: 'anthropic' }])} turnNumber={1} showMarker />
      </TranscriptViewProvider>,
    );
    expect(screen.queryByText('long reasoning body')).toBeNull();
  });

  it('redacted thinking (text null) renders a muted marker, not an empty block', () => {
    render(<Turn turn={turnOf([{ kind: 'thinking', text: null, provider: 'anthropic' }])} turnNumber={1} showMarker />);
    expect(screen.getByText(/redacted reasoning/)).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /thinking/ })).toBeNull();
  });

  it('unknown block renders the labelled fallback with raw data', () => {
    render(<Turn turn={turnOf([{ kind: 'unknown', type: 'brand_new_event', data: { x: 1 } }])} turnNumber={1} showMarker />);
    expect(screen.getByText(/unrecognized event: brand_new_event/)).toBeInTheDocument();
    expect(screen.getByText(/"x": 1/)).toBeInTheDocument();
  });

  it('a Findings filter hides a turn with no findings entirely', () => {
    const { container } = render(
      <TranscriptViewProvider value={{ filter: 'findings', expandTarget: null, expandNonce: 0, live: false }}>
        <Turn turn={basic} turnNumber={1} showMarker />
      </TranscriptViewProvider>,
    );
    expect(container).toBeEmptyDOMElement();
  });

  it('renders every quiet block kind', () => {
    render(
      <Turn
        turnNumber={1}
        showMarker
        turn={turnOf([
          { kind: 'seed', messageCount: 4, sourceTranscript: null },
          { kind: 'user', content: 'do the task' },
          { kind: 'gate', gateId: 'g1', prompt: 'ship it?', decision: null, decidedBy: null },
          { kind: 'notice', noticeKind: 'context_trim', message: 'trimmed to fit' },
          { kind: 'meta', label: 'skills', text: 'loaded skill pdf' },
          { kind: 'compaction', seq: 1, summarized: 9 },
          { kind: 'outcome', severity: 'error', title: 'refused · cyber', detail: 'Declined.' },
          { kind: 'recovery', text: '↺ rung 1 · retried' },
          { kind: 'assistant_block', text: 'served by fallback', abandoned: false, raw: { to_model: 'model-b' } },
        ])}
      />,
    );
    for (const re of [/4 prior messages/, /do the task/, /ship it\?/, /trimmed to fit/, /loaded skill pdf/, /summarized 9/, /refused · cyber/, /Declined\./, /rung 1/, /served by fallback/]) {
      expect(screen.getByText(re)).toBeInTheDocument();
    }
  });
});
