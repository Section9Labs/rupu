import { describe, it, expect } from 'vitest';
import { buildTranscriptView } from './transcriptView';
import type { TranscriptEvent } from '../../lib/transcript';

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

const RUN_START: TranscriptEvent = {
  type: 'run_start',
  data: {
    run_id: 'r1',
    agent: 'assess',
    provider: 'oracle-assessor',
    model: 'claude-mythos-preview',
    started_at: '2026-06-18T00:00:00Z',
    mode: 'ask',
  },
};

const ASSISTANT: TranscriptEvent = {
  type: 'assistant_message',
  data: {
    content: "I'll read the keyring module and trace how the AES key is sourced.",
    thinking: 'trace untrusted input to key material; check for hardcoded secrets…',
  },
};

const TOOL_CALL: TranscriptEvent = {
  type: 'tool_call',
  data: { call_id: 'c1', tool: 'read_file', input: { path: 'keyring.rs', range: '1-80' } },
};

const TOOL_RESULT: TranscriptEvent = {
  type: 'tool_result',
  data: { call_id: 'c1', output: '80 lines · const KEY:[u8;32]=…', duration_ms: 120 },
};

const RUN_COMPLETE: TranscriptEvent = {
  type: 'run_complete',
  data: { run_id: 'r1', status: 'completed', total_tokens: 4210, duration_ms: 38000 },
};

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

describe('buildTranscriptView', () => {
  it('builds header, paired tool, thinking, and footer from a full stream', () => {
    const view = buildTranscriptView([RUN_START, ASSISTANT, TOOL_CALL, TOOL_RESULT, RUN_COMPLETE]);

    // header from run_start
    expect(view.header).not.toBeNull();
    expect(view.header?.agent).toBe('assess');
    expect(view.header?.model).toBe('claude-mythos-preview');
    expect(view.header?.provider).toBe('oracle-assessor');

    // one assistant item carrying the thinking
    const assistants = view.items.filter((i) => i.kind === 'assistant');
    expect(assistants).toHaveLength(1);
    const asst = assistants[0];
    if (asst.kind !== 'assistant') throw new Error('expected assistant');
    expect(asst.content).toContain('keyring module');
    expect(asst.thinking).toContain('hardcoded secrets');

    // exactly one tool item, with the call paired to its result by call_id
    const tools = view.items.filter((i) => i.kind === 'tool');
    expect(tools).toHaveLength(1);
    const tool = tools[0];
    if (tool.kind !== 'tool') throw new Error('expected tool');
    expect(tool.callId).toBe('c1');
    expect(tool.tool).toBe('read_file');
    expect(tool.result).not.toBeNull();
    expect(tool.result?.output).toContain('const KEY');
    expect(tool.result?.durationMs).toBe(120);
    expect(tool.result?.error).toBeNull();

    // footer from run_complete
    expect(view.footer).not.toBeNull();
    expect(view.footer?.status).toBe('completed');
    expect(view.footer?.totalTokens).toBe(4210);
    expect(view.footer?.durationMs).toBe(38000);
  });

  it('renders an unpaired tool_result as its own item without crashing', () => {
    const orphan: TranscriptEvent = {
      type: 'tool_result',
      data: { call_id: 'missing', output: 'orphan output', duration_ms: 5 },
    };
    const view = buildTranscriptView([RUN_START, orphan]);

    const tools = view.items.filter((i) => i.kind === 'tool');
    expect(tools).toHaveLength(1);
    const tool = tools[0];
    if (tool.kind !== 'tool') throw new Error('expected tool');
    // no matching call → tool name unknown, but the result is surfaced
    expect(tool.callId).toBe('missing');
    expect(tool.result?.output).toBe('orphan output');
  });

  it('handles an empty stream', () => {
    const view = buildTranscriptView([]);
    expect(view.header).toBeNull();
    expect(view.footer).toBeNull();
    expect(view.items).toHaveLength(0);
  });

  it('derives footer from a usage event when run_complete is absent', () => {
    const usage: TranscriptEvent = {
      type: 'usage',
      data: { input_tokens: 4210, output_tokens: 880, cached_tokens: 0 },
    };
    const view = buildTranscriptView([RUN_START, ASSISTANT, usage]);
    expect(view.footer).not.toBeNull();
    expect(view.footer?.totalTokens).toBe(5090);
  });

  it('captures user messages from turn events', () => {
    const userTurn: TranscriptEvent = {
      type: 'user_message',
      data: { content: 'Assess crypto-svc/keyring.rs…' },
    };
    const view = buildTranscriptView([RUN_START, userTurn, ASSISTANT]);
    const users = view.items.filter((i) => i.kind === 'user');
    expect(users).toHaveLength(1);
    const u = users[0];
    if (u.kind !== 'user') throw new Error('expected user');
    expect(u.content).toContain('keyring.rs');
  });
});
