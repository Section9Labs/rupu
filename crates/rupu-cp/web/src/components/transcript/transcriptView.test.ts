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

const RUN_COMPLETE: TranscriptEvent = {
  type: 'run_complete',
  data: { run_id: 'r1', status: 'completed', total_tokens: 4210, duration_ms: 38000 },
};

function readFileCall(callId: string, path: string): TranscriptEvent {
  return { type: 'tool_call', data: { call_id: callId, tool: 'read_file', input: { path } } };
}

function toolResult(callId: string, output: string, durationMs = 10): TranscriptEvent {
  return { type: 'tool_result', data: { call_id: callId, output, duration_ms: durationMs } };
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

describe('buildTranscriptView — header / footer', () => {
  it('builds the header from run_start', () => {
    const view = buildTranscriptView([RUN_START]);
    expect(view.header).not.toBeNull();
    expect(view.header?.agent).toBe('assess');
    expect(view.header?.model).toBe('claude-mythos-preview');
    expect(view.header?.provider).toBe('oracle-assessor');
    expect(view.header?.mode).toBe('ask');
    expect(view.header?.startedAt).toBe('2026-06-18T00:00:00Z');
  });

  it('builds the footer from run_complete', () => {
    const view = buildTranscriptView([RUN_START, RUN_COMPLETE]);
    expect(view.footer).not.toBeNull();
    expect(view.footer?.status).toBe('completed');
    expect(view.footer?.totalTokens).toBe(4210);
    expect(view.footer?.durationMs).toBe(38000);
  });

  it('derives a footer from usage when run_complete is absent', () => {
    const usage: TranscriptEvent = {
      type: 'usage',
      data: { input_tokens: 4210, output_tokens: 880, cached_tokens: 0 },
    };
    const view = buildTranscriptView([RUN_START, ASSISTANT, usage]);
    expect(view.footer).not.toBeNull();
    expect(view.footer?.totalTokens).toBe(5090);
    expect(view.footer?.status).toBeNull();
  });

  it('handles an empty stream', () => {
    const view = buildTranscriptView([]);
    expect(view.header).toBeNull();
    expect(view.footer).toBeNull();
    expect(view.turns).toHaveLength(0);
  });
});

describe('buildTranscriptView — findings from report_finding', () => {
  const FINDING_CALL: TranscriptEvent = {
    type: 'tool_call',
    data: {
      call_id: 'f1',
      tool: 'report_finding',
      input: {
        severity: 'high',
        summary: 'Hardcoded AES key in keyring.rs',
        scope: 'file',
        file_path: 'keyring.rs',
        line_range: [12, 14],
        concern_id: 'crypto-1',
        evidence: {
          code_excerpt: 'const KEY: [u8; 32] = [0x00; 32];',
          rationale: 'A static key defeats encryption at rest.',
          references: ['CWE-321'],
        },
      },
    },
  };
  const FINDING_RESULT: TranscriptEvent = {
    type: 'tool_result',
    data: { call_id: 'f1', output: 'recorded', duration_ms: 3 },
  };

  it('produces one finding ToolView with parsed fields', () => {
    const view = buildTranscriptView([RUN_START, ASSISTANT, FINDING_CALL, FINDING_RESULT]);
    const tools = view.turns.flatMap((t) => t.tools);
    expect(tools).toHaveLength(1);
    const tool = tools[0];
    expect(tool.kind).toBe('finding');
    expect(tool.tool).toBe('report_finding');
    expect(tool.finding).toBeDefined();
    expect(tool.finding?.severity).toBe('high');
    expect(tool.finding?.summary).toBe('Hardcoded AES key in keyring.rs');
    expect(tool.finding?.scope).toBe('file');
    expect(tool.finding?.filePath).toBe('keyring.rs');
    expect(tool.finding?.lineRange).toEqual([12, 14]);
    expect(tool.finding?.concernId).toBe('crypto-1');
    expect(tool.finding?.rationale).toBe('A static key defeats encryption at rest.');
    expect(tool.finding?.codeExcerpt).toBe('const KEY: [u8; 32] = [0x00; 32];');
    expect(tool.finding?.references).toEqual(['CWE-321']);
  });

  it('yields a finding from the tool_call ALONE (no action_emitted, no result)', () => {
    const view = buildTranscriptView([RUN_START, ASSISTANT, FINDING_CALL]);
    const tools = view.turns.flatMap((t) => t.tools);
    expect(tools).toHaveLength(1);
    expect(tools[0].kind).toBe('finding');
    expect(tools[0].finding?.severity).toBe('high');
    expect(view.turns[0].summary.findingCount).toBe(1);
  });
});

describe('buildTranscriptView — terminal / diff pairing', () => {
  it('pairs a following command_run onto a bash tool, using argv[2]', () => {
    const bashCall: TranscriptEvent = {
      type: 'tool_call',
      data: { call_id: 'b1', tool: 'bash', input: { command: 'ls -la' } },
    };
    const cmdRun: TranscriptEvent = {
      type: 'command_run',
      data: { argv: ['/bin/sh', '-c', 'ls -la'], cwd: '/x', exit_code: 0, stdout_bytes: 10, stderr_bytes: 0 },
    };
    const view = buildTranscriptView([RUN_START, ASSISTANT, bashCall, cmdRun]);
    const tools = view.turns.flatMap((t) => t.tools);
    expect(tools).toHaveLength(1);
    const tool = tools[0];
    expect(tool.kind).toBe('terminal');
    expect(tool.terminal?.command).toBe('ls -la');
    expect(tool.terminal?.exitCode).toBe(0);
    expect(tool.terminal?.cwd).toBe('/x');
  });

  it('pairs a following file_edit onto an edit_file tool', () => {
    const editCall: TranscriptEvent = {
      type: 'tool_call',
      data: { call_id: 'e1', tool: 'edit_file', input: { path: 'a.rs' } },
    };
    const fileEdit: TranscriptEvent = {
      type: 'file_edit',
      data: { path: 'a.rs', kind: 'modify', diff: '@@ -1 +1 @@\n-old\n+new' },
    };
    const view = buildTranscriptView([RUN_START, ASSISTANT, editCall, fileEdit]);
    const tools = view.turns.flatMap((t) => t.tools);
    expect(tools).toHaveLength(1);
    const tool = tools[0];
    expect(tool.kind).toBe('diff');
    expect(tool.diff?.path).toBe('a.rs');
    expect(tool.diff?.editKind).toBe('modify');
    expect(tool.diff?.diff).toContain('+new');
  });
});

describe('buildTranscriptView — tool kinds', () => {
  function kindOf(tool: string, input: unknown = {}): string {
    const call: TranscriptEvent = { type: 'tool_call', data: { call_id: 'k1', tool, input } };
    const view = buildTranscriptView([RUN_START, ASSISTANT, call]);
    return view.turns.flatMap((t) => t.tools)[0].kind;
  }

  it('maps tool names to kinds', () => {
    expect(kindOf('read_file')).toBe('read');
    expect(kindOf('grep')).toBe('grep');
    expect(kindOf('glob')).toBe('glob');
    expect(kindOf('dispatch_agent')).toBe('subrun');
    expect(kindOf('dispatch_agents_parallel')).toBe('subrun');
    expect(kindOf('coverage_status')).toBe('coverage');
    expect(kindOf('coverage_remaining')).toBe('coverage');
    expect(kindOf('something_else')).toBe('generic');
  });
});

describe('buildTranscriptView — ast_grep tool kind + structured payload', () => {
  it('classifies ast_grep as its own kind and carries structured payload', () => {
    const events = [
      RUN_START,
      ASSISTANT,
      { type: 'tool_call', data: { call_id: 'c1', tool: 'ast_grep', input: { pattern: 'impl $T for $S', lang: 'rust' } } },
      { type: 'tool_result', data: { call_id: 'c1', output: 'a.rs:1:1: impl X for Y', duration_ms: 3, structured: { tool: 'ast_grep', matchCount: 1, matches: [] } } },
    ];
    const view = buildTranscriptView(events as unknown as TranscriptEvent[]);
    const tool = view.turns.flatMap((t) => t.tools).find((x) => x.tool === 'ast_grep')!;
    expect(tool.kind).toBe('ast_grep');
    expect((tool.structured as { matchCount: number }).matchCount).toBe(1);
  });
});

describe('buildTranscriptView — turn grouping', () => {
  it('groups tools under their preceding assistant message', () => {
    const finding: TranscriptEvent = {
      type: 'tool_call',
      data: {
        call_id: 'f1',
        tool: 'report_finding',
        input: { severity: 'low', summary: 's', scope: 'file', evidence: { rationale: 'r' } },
      },
    };
    const asstB: TranscriptEvent = { type: 'assistant_message', data: { content: 'B' } };
    const view = buildTranscriptView([
      RUN_START,
      ASSISTANT,
      readFileCall('c1', 'x.rs'),
      toolResult('c1', 'out'),
      asstB,
      finding,
      toolResult('f1', 'recorded'),
      RUN_COMPLETE,
    ]);

    expect(view.turns).toHaveLength(2);

    const t0 = view.turns[0];
    expect(t0.assistant?.content).toContain('keyring module');
    expect(t0.tools).toHaveLength(1);
    expect(t0.tools[0].kind).toBe('read');
    expect(t0.summary.toolCount).toBe(1);
    expect(t0.summary.findingCount).toBe(0);
    expect(t0.summary.result).toBe('ok');

    const t1 = view.turns[1];
    expect(t1.assistant?.content).toBe('B');
    expect(t1.tools).toHaveLength(1);
    expect(t1.tools[0].kind).toBe('finding');
    expect(t1.summary.findingCount).toBe(1);
  });

  it('puts tools before the first assistant_message in a leading turn', () => {
    const view = buildTranscriptView([
      RUN_START,
      readFileCall('c1', 'x.rs'),
      toolResult('c1', 'out'),
      ASSISTANT,
    ]);
    expect(view.turns).toHaveLength(2);
    expect(view.turns[0].assistant).toBeUndefined();
    expect(view.turns[0].tools).toHaveLength(1);
    expect(view.turns[1].assistant?.content).toContain('keyring module');
    expect(view.turns[1].tools).toHaveLength(0);
  });

  it("summary.result is 'error' when any tool errored", () => {
    const call = readFileCall('c1', 'x.rs');
    const errResult: TranscriptEvent = {
      type: 'tool_result',
      data: { call_id: 'c1', output: '', error: 'ENOENT', duration_ms: 1 },
    };
    const view = buildTranscriptView([RUN_START, ASSISTANT, call, errResult, RUN_COMPLETE]);
    expect(view.turns[0].summary.result).toBe('error');
    expect(view.turns[0].tools[0].error).toBe('ENOENT');
  });

  it("summary.result is 'running' when no run_complete is seen", () => {
    const view = buildTranscriptView([RUN_START, ASSISTANT, readFileCall('c1', 'x.rs')]);
    expect(view.turns[0].summary.result).toBe('running');
  });
});

describe('buildTranscriptView — graceful ignores', () => {
  it('ignores phantom user_message / action_emitted events without a phantom item', () => {
    const userMsg: TranscriptEvent = {
      type: 'user_message',
      data: { content: 'hello' },
    };
    const actionEmitted: TranscriptEvent = {
      type: 'action_emitted',
      data: { action: 'report_finding', severity: 'high', summary: 'legacy' },
    };
    const view = buildTranscriptView([RUN_START, userMsg, actionEmitted, ASSISTANT]);
    const tools = view.turns.flatMap((t) => t.tools);
    expect(tools).toHaveLength(0);
    // exactly one assistant turn, no phantom finding from action_emitted
    expect(view.turns).toHaveLength(1);
    expect(view.turns[0].assistant?.content).toContain('keyring module');
  });
});

describe('buildTranscriptView — result pairing by call_id', () => {
  it('attaches output / durationMs / error onto the matching tool', () => {
    const view = buildTranscriptView([
      RUN_START,
      ASSISTANT,
      readFileCall('c1', 'keyring.rs'),
      toolResult('c1', '80 lines · const KEY', 120),
      RUN_COMPLETE,
    ]);
    const tool = view.turns.flatMap((t) => t.tools)[0];
    expect(tool.callId).toBe('c1');
    expect(tool.output).toContain('const KEY');
    expect(tool.durationMs).toBe(120);
    expect(tool.error).toBeUndefined();
  });
});
