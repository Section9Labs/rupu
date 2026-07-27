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

  it('still ignores action_emitted even alongside a tool_audit line (regression: never conflate the two)', () => {
    const actionEmitted: TranscriptEvent = {
      type: 'action_emitted',
      data: { kind: 'scm.prs.comment', payload: {}, allowed: true, applied: true },
    };
    const audit: TranscriptEvent = {
      type: 'tool_audit',
      data: { tool: 'scm.prs.comment', declared: true, granted: true, blocked: false, restricted: true },
    };
    const view = buildTranscriptView([actionEmitted, audit]);
    const tools = view.turns.flatMap((t) => t.tools);
    // Only the tool_audit produces a visible item; action_emitted stays dead.
    expect(tools).toHaveLength(1);
    expect(tools[0].tool).toBe('scm.prs.comment');
    expect(tools[0].audit).toBeDefined();
  });
});

describe('buildTranscriptView — tool_audit', () => {
  function auditEvent(overrides: Partial<{ tool: string; declared: boolean; granted: boolean; blocked: boolean; restricted: boolean }> = {}): TranscriptEvent {
    return {
      type: 'tool_audit',
      data: {
        tool: 'issues.create',
        declared: false,
        granted: true,
        blocked: false,
        restricted: false,
        ...overrides,
      },
    };
  }

  it('pairs a tool_audit line onto the preceding tool_call (agent path)', () => {
    const call: TranscriptEvent = {
      type: 'tool_call',
      data: { call_id: 'c1', tool: 'issues.create', input: { title: 'x' } },
    };
    const result: TranscriptEvent = {
      type: 'tool_result',
      data: { call_id: 'c1', output: '{"id":1}', duration_ms: 5 },
    };
    const view = buildTranscriptView([RUN_START, ASSISTANT, call, auditEvent(), result]);
    const tools = view.turns.flatMap((t) => t.tools);
    expect(tools).toHaveLength(1);
    expect(tools[0].tool).toBe('issues.create');
    expect(tools[0].audit).toEqual({ declared: false, granted: true, blocked: false, restricted: false });
    // The paired result still attaches normally.
    expect(tools[0].output).toBe('{"id":1}');
  });

  it('marks a blocked:true audit distinctly from an allowed one', () => {
    const call: TranscriptEvent = {
      type: 'tool_call',
      data: { call_id: 'c1', tool: 'scm.prs.diff', input: {} },
    };
    const view = buildTranscriptView([
      RUN_START,
      ASSISTANT,
      call,
      auditEvent({ tool: 'scm.prs.diff', declared: true, granted: false, blocked: true, restricted: true }),
    ]);
    const tool = view.turns.flatMap((t) => t.tools)[0];
    expect(tool.audit?.blocked).toBe(true);
    expect(tool.audit?.granted).toBe(false);
    expect(tool.audit?.declared).toBe(true);
  });

  it('surfaces a standalone tool_audit with no preceding tool_call (action-node path)', () => {
    // execute_action_step's transcript has ONLY action_emitted + tool_audit
    // lines — no tool_call/tool_result shape at all. The audit must still
    // render, not be silently dropped for lack of a pairing target.
    const view = buildTranscriptView([
      auditEvent({ tool: 'scm.prs.comment', declared: true, granted: true, blocked: false, restricted: true }),
    ]);
    const tools = view.turns.flatMap((t) => t.tools);
    expect(tools).toHaveLength(1);
    expect(tools[0].tool).toBe('scm.prs.comment');
    expect(tools[0].audit).toEqual({ declared: true, granted: true, blocked: false, restricted: true });
  });

  it('attributes each audit to its own card when a turn has 2 tool_calls (disk order: call A, call B, audit A, result A, audit B, result B)', () => {
    // Reproduces run_agent's real on-disk ordering: ALL of a turn's
    // tool_call events are written up front (the "collect tool_uses"
    // loop), THEN the dispatch loop runs each in sequence emitting that
    // call's audit + result before moving to the next. A naive
    // single-slot / last-call-wins pairing scheme attaches call A's audit
    // to call B's card and spills call B's audit into a third, phantom
    // standalone card.
    const callA: TranscriptEvent = {
      type: 'tool_call',
      data: { call_id: 'a1', tool: 'issues.create', input: { title: 'A' } },
    };
    const callB: TranscriptEvent = {
      type: 'tool_call',
      data: { call_id: 'b1', tool: 'scm.prs.comment', input: { body: 'B' } },
    };
    const auditA = auditEvent({ tool: 'issues.create', declared: true, granted: true, blocked: false, restricted: true });
    const resultA: TranscriptEvent = { type: 'tool_result', data: { call_id: 'a1', output: 'ok-a', duration_ms: 1 } };
    const auditB = auditEvent({ tool: 'scm.prs.comment', declared: false, granted: true, blocked: true, restricted: true });
    const resultB: TranscriptEvent = { type: 'tool_result', data: { call_id: 'b1', output: '', error: 'permission_denied', duration_ms: 1 } };

    const view = buildTranscriptView([
      RUN_START,
      ASSISTANT,
      callA,
      callB,
      auditA,
      resultA,
      auditB,
      resultB,
    ]);
    const tools = view.turns.flatMap((t) => t.tools);
    // Exactly 2 cards — no third, phantom standalone card.
    expect(tools).toHaveLength(2);

    const cardA = tools.find((t) => t.tool === 'issues.create')!;
    expect(cardA.audit).toEqual({ declared: true, granted: true, blocked: false, restricted: true });
    expect(cardA.output).toBe('ok-a');

    const cardB = tools.find((t) => t.tool === 'scm.prs.comment')!;
    expect(cardB.audit).toEqual({ declared: false, granted: true, blocked: true, restricted: true });
    expect(cardB.error).toBe('permission_denied');
  });

  it('attributes 2 audits for the SAME tool called twice in one turn, FIFO', () => {
    const call1: TranscriptEvent = { type: 'tool_call', data: { call_id: 'x1', tool: 'issues.comment', input: { body: '1' } } };
    const call2: TranscriptEvent = { type: 'tool_call', data: { call_id: 'x2', tool: 'issues.comment', input: { body: '2' } } };
    const audit1 = auditEvent({ tool: 'issues.comment', declared: true, granted: true, blocked: false, restricted: true });
    const result1: TranscriptEvent = { type: 'tool_result', data: { call_id: 'x1', output: 'r1', duration_ms: 1 } };
    const audit2 = auditEvent({ tool: 'issues.comment', declared: true, granted: true, blocked: true, restricted: true });
    const result2: TranscriptEvent = { type: 'tool_result', data: { call_id: 'x2', output: '', error: 'permission_denied', duration_ms: 1 } };

    const view = buildTranscriptView([
      RUN_START,
      ASSISTANT,
      call1,
      call2,
      audit1,
      result1,
      audit2,
      result2,
    ]);
    const tools = view.turns.flatMap((t) => t.tools);
    expect(tools).toHaveLength(2);
    expect(tools[0].callId).toBe('x1');
    expect(tools[0].audit?.blocked).toBe(false);
    expect(tools[1].callId).toBe('x2');
    expect(tools[1].audit?.blocked).toBe(true);
  });

  it('does not carry an audit onto an unrelated later tool_call', () => {
    const call1: TranscriptEvent = { type: 'tool_call', data: { call_id: 'c1', tool: 'issues.list', input: {} } };
    const call2: TranscriptEvent = { type: 'tool_call', data: { call_id: 'c2', tool: 'issues.get', input: {} } };
    const view = buildTranscriptView([
      RUN_START,
      ASSISTANT,
      call1,
      auditEvent({ tool: 'issues.list' }),
      call2,
    ]);
    const tools = view.turns.flatMap((t) => t.tools);
    expect(tools).toHaveLength(2);
    expect(tools[0].tool).toBe('issues.list');
    expect(tools[0].audit).toBeDefined();
    expect(tools[1].tool).toBe('issues.get');
    expect(tools[1].audit).toBeUndefined();
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
