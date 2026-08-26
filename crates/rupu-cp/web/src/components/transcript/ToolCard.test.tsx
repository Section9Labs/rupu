// @vitest-environment jsdom
/**
 * Tests for ToolCard:
 *   1. summarizeInput helper — pure, no DOM (required)
 *   2. Smoke renders via testing-library (optional, per spec)
 */

import { it, expect, describe, afterEach, vi } from 'vitest';
import { render, screen, cleanup, waitFor } from '@testing-library/react';
import { summarizeInput, parseAstGrepText, parseSubrunOutput } from './ToolCard';
import ToolCard from './ToolCard';
import type { ToolView, FindingView } from './transcriptView';
import { api } from '../../lib/api';
import type { SourceSlice, AstResponse } from '../../lib/api';

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function makeToolView(overrides: Partial<ToolView> & Pick<ToolView, 'tool' | 'kind'>): ToolView {
  return {
    input: {},
    ...overrides,
  };
}

// ---------------------------------------------------------------------------
// 1. summarizeInput unit tests (pure helper — no DOM)
// ---------------------------------------------------------------------------

describe('summarizeInput', () => {
  it('read — returns path when no line range', () => {
    const tv = makeToolView({
      tool: 'read_file',
      kind: 'read',
      input: { path: 'src/lib/api.ts' },
    });
    expect(summarizeInput(tv)).toBe('src/lib/api.ts');
  });

  it('read — appends start-end range when both present', () => {
    const tv = makeToolView({
      tool: 'read_file',
      kind: 'read',
      input: { path: 'src/lib/api.ts', start_line: 10, end_line: 20 },
    });
    expect(summarizeInput(tv)).toBe('src/lib/api.ts:10-20');
  });

  it('read — appends start only when end absent', () => {
    const tv = makeToolView({
      tool: 'read_file',
      kind: 'read',
      input: { path: 'src/lib/api.ts', start_line: 42 },
    });
    expect(summarizeInput(tv)).toBe('src/lib/api.ts:42');
  });

  it('grep — returns pattern + path when both present', () => {
    const tv = makeToolView({
      tool: 'grep',
      kind: 'grep',
      input: { pattern: 'ToolView', path: 'src/' },
    });
    expect(summarizeInput(tv)).toBe('ToolView  src/');
  });

  it('grep — returns only pattern when path absent', () => {
    const tv = makeToolView({
      tool: 'grep',
      kind: 'grep',
      input: { pattern: 'myFunc' },
    });
    expect(summarizeInput(tv)).toBe('myFunc');
  });

  it('glob — returns pattern', () => {
    const tv = makeToolView({
      tool: 'glob',
      kind: 'glob',
      input: { pattern: '**/*.tsx' },
    });
    expect(summarizeInput(tv)).toBe('**/*.tsx');
  });

  it('terminal — returns command (truncated if long)', () => {
    const short = makeToolView({
      tool: 'bash',
      kind: 'terminal',
      input: { command: 'npm test' },
    });
    expect(summarizeInput(short)).toBe('npm test');

    const longCmd = 'x'.repeat(70);
    const long = makeToolView({
      tool: 'bash',
      kind: 'terminal',
      input: { command: longCmd },
    });
    const result = summarizeInput(long);
    // '…' is a single character so length is 57 + 1 = 58
    expect(result.length).toBe(58);
    expect(result.endsWith('…')).toBe(true);
  });

  it('generic — returns empty string when input has no recognisable key', () => {
    const tv = makeToolView({
      tool: 'some_tool',
      kind: 'generic',
      input: { foo: 42, bar: true },
    });
    expect(summarizeInput(tv)).toBe('');
  });

  it('generic — returns the "name" field when present', () => {
    const tv = makeToolView({
      tool: 'some_tool',
      kind: 'generic',
      input: { name: 'my-workflow' },
    });
    expect(summarizeInput(tv)).toBe('my-workflow');
  });

  it('returns empty string when input is null', () => {
    const tv = makeToolView({ tool: 'read_file', kind: 'read', input: null });
    expect(summarizeInput(tv)).toBe('');
  });

  it('returns the string itself when input is a string', () => {
    const tv = makeToolView({ tool: 'some_tool', kind: 'generic', input: 'raw string' });
    expect(summarizeInput(tv)).toBe('raw string');
  });

  it('ast_grep — returns pattern · lang when both present', () => {
    const tv = makeToolView({
      tool: 'ast_grep',
      kind: 'ast_grep',
      input: { pattern: 'impl $T for $S', lang: 'rust' },
    });
    expect(summarizeInput(tv)).toBe('impl $T for $S · rust');
  });

  it('ast_grep — returns only pattern when lang absent', () => {
    const tv = makeToolView({
      tool: 'ast_grep',
      kind: 'ast_grep',
      input: { pattern: 'impl $T for $S' },
    });
    expect(summarizeInput(tv)).toBe('impl $T for $S');
  });

  it('ast_grep — falls back to lang alone when pattern absent (mirrors grep)', () => {
    const tv = makeToolView({
      tool: 'ast_grep',
      kind: 'ast_grep',
      input: { lang: 'rust' },
    });
    expect(summarizeInput(tv)).toBe('rust');
  });

  it('ast_grep — returns empty string when both pattern and lang absent', () => {
    const tv = makeToolView({
      tool: 'ast_grep',
      kind: 'ast_grep',
      input: {},
    });
    expect(summarizeInput(tv)).toBe('');
  });
});

// ---------------------------------------------------------------------------
// 2. Smoke renders (testing-library)
// ---------------------------------------------------------------------------

const FINDING: FindingView = {
  severity: 'high',
  summary: 'Secret key in binary',
  scope: 'file',
  rationale: 'Hardcoded secret.',
  references: [],
};

it('finding ToolView renders the finding summary (no extra outer header)', () => {
  const tv: ToolView = {
    tool: 'report_finding',
    kind: 'finding',
    input: {},
    finding: FINDING,
  };
  render(<ToolCard tool={tv} />);
  expect(screen.getByText('Secret key in binary')).not.toBeNull();
  // Finding uses its own chrome — no ⚙ prefix in the header
  expect(screen.queryByText(/⚙ report_finding/)).toBeNull();
});

it('terminal ToolView renders the command inside TerminalBlock', () => {
  const tv: ToolView = {
    tool: 'bash',
    kind: 'terminal',
    input: { command: 'cargo test' },
    output: 'test result: ok',
    terminal: { command: 'cargo test', cwd: '/repo', exitCode: 0 },
  };
  render(<ToolCard tool={tv} />);
  // The command appears in both the header summary and TerminalBlock prompt —
  // use getAllByText and assert we get at least one match.
  expect(screen.getAllByText('cargo test').length).toBeGreaterThan(0);
  expect(screen.getByText('test result: ok')).not.toBeNull();
});

it('generic ToolView with JSON output renders KV (not [object Object], not raw JSON)', () => {
  const tv: ToolView = {
    tool: 'my_tool',
    kind: 'generic',
    input: {},
    output: JSON.stringify({ status: 'done', count: 3 }),
  };
  render(<ToolCard tool={tv} />);
  // StructuredView should render the keys
  expect(screen.getByText('status')).not.toBeNull();
  expect(screen.getByText('done')).not.toBeNull();
  expect(screen.getByText('count')).not.toBeNull();
  expect(screen.getByText('3')).not.toBeNull();
  // Must NOT render raw JSON or [object Object]
  expect(screen.queryByText('[object Object]')).toBeNull();
  expect(screen.queryByText(/^\{"status":/)).toBeNull();
});

it('error ToolView shows red error block', () => {
  const tv: ToolView = {
    tool: 'read_file',
    kind: 'read',
    input: { path: 'missing.ts' },
    error: 'File not found: missing.ts',
  };
  render(<ToolCard tool={tv} />);
  expect(screen.getByText('Error')).not.toBeNull();
  expect(screen.getByText('File not found: missing.ts')).not.toBeNull();
});

// The subrun output fixtures below mirror the ACTUAL wire shape emitted by
// crates/rupu-tools/src/dispatch_agent.rs (single: top-level `ok` /
// `tokens_used` / `transcript_path` / `sub_run_id`) and
// dispatch_agents_parallel.rs (top-level `ok` + `all_succeeded`, per-request
// fields nested under `results` keyed by request id). The previous fixtures
// used `status` / `total_tokens`, which no producer emits.

it('subrun ToolView (dispatch_agent shape) renders ok + token chips and transcript button', () => {
  let clicked = '';
  const tv: ToolView = {
    tool: 'dispatch_agent',
    kind: 'subrun',
    input: { agent: 'scanner' },
    output: JSON.stringify({
      ok: true,
      agent: 'scanner',
      output: 'done',
      findings: [],
      tokens_used: 4200,
      duration_ms: 500,
      transcript_path: '/runs/abc/transcript.jsonl',
      sub_run_id: 'sub_abc',
    }),
  };
  render(<ToolCard tool={tv} onOpenTranscript={(p) => { clicked = p; }} />);
  // The card header has its own muted "ok" chip; the body's status badge is
  // the green-toned one.
  expect(screen.getAllByText('ok').some((el) => el.className.includes('bg-ok-bg'))).toBe(true);
  expect(screen.getByText('4,200 tokens')).not.toBeNull();
  const btn = screen.getByRole('button', { name: /View sub-run transcript/ });
  expect(btn).not.toBeNull();
  btn.click();
  expect(clicked).toBe('/runs/abc/transcript.jsonl');
});

it('subrun ToolView with ok:false renders a failed chip', () => {
  const tv: ToolView = {
    tool: 'dispatch_agent',
    kind: 'subrun',
    input: { agent: 'scanner' },
    output: JSON.stringify({
      ok: false,
      agent: 'scanner',
      output: 'boom',
      findings: [],
      tokens_used: 0,
      duration_ms: 10,
      transcript_path: '/runs/x/transcript.jsonl',
      sub_run_id: 'sub_x',
    }),
  };
  render(<ToolCard tool={tv} />);
  expect(screen.getByText('failed')).not.toBeNull();
  expect(screen.getByText('0 tokens')).not.toBeNull();
});

it('subrun ToolView without callback renders path as chip (no button)', () => {
  const tv: ToolView = {
    tool: 'dispatch_agent',
    kind: 'subrun',
    input: { agent: 'scanner' },
    output: JSON.stringify({
      ok: true,
      tokens_used: 12,
      transcript_path: '/runs/abc/transcript.jsonl',
      sub_run_id: 'sub_abc',
    }),
  };
  render(<ToolCard tool={tv} />);
  // No button
  expect(screen.queryByRole('button', { name: /View sub-run transcript/ })).toBeNull();
  // Path shown as chip text
  expect(screen.getByText('/runs/abc/transcript.jsonl')).not.toBeNull();
});

it('subrun ToolView (dispatch_agents_parallel shape) renders per-request rows', () => {
  const opened: string[] = [];
  const tv: ToolView = {
    tool: 'dispatch_agents_parallel',
    kind: 'subrun',
    input: { requests: [] },
    output: JSON.stringify({
      ok: false,
      results: {
        reviewer: {
          ok: true,
          agent: 'reviewer',
          output: 'lgtm',
          findings: [],
          tokens_used: 1500,
          duration_ms: 900,
          transcript_path: '/runs/p/sub_r/transcript.jsonl',
          sub_run_id: 'sub_r',
        },
        scanner: {
          ok: false,
          agent: 'scanner',
          error: 'dispatch failed: agent not found',
        },
      },
      all_succeeded: false,
    }),
  };
  render(<ToolCard tool={tv} onOpenTranscript={(p) => { opened.push(p); }} />);
  // Per-request ids shown
  expect(screen.getByText('reviewer')).not.toBeNull();
  expect(screen.getByText('scanner')).not.toBeNull();
  // reviewer row: ok chip (green-toned; the header has its own muted "ok"
  // chip) + tokens + working transcript button
  expect(screen.getAllByText('ok').some((el) => el.className.includes('bg-ok-bg'))).toBe(true);
  expect(screen.getByText('1,500 tokens')).not.toBeNull();
  const btn = screen.getByRole('button', { name: /View sub-run transcript/ });
  btn.click();
  expect(opened).toEqual(['/runs/p/sub_r/transcript.jsonl']);
  // scanner row: failed chips (top-level + row) + error text
  expect(screen.getAllByText('failed').length).toBe(2);
  expect(screen.getByText(/agent not found/)).not.toBeNull();
});

it('subrun ToolView with unparseable output falls back to raw pre', () => {
  const tv: ToolView = {
    tool: 'dispatch_agent',
    kind: 'subrun',
    input: { agent: 'scanner' },
    output: 'not json at all',
  };
  render(<ToolCard tool={tv} />);
  expect(screen.getByText('not json at all')).not.toBeNull();
});

describe('parseSubrunOutput', () => {
  it('parses a real dispatch_agent result body', () => {
    const json = JSON.stringify({
      ok: true, agent: 'reviewer', output: 'done', findings: [],
      tokens_used: 1234, duration_ms: 500,
      transcript_path: '/tmp/sub_TEST/transcript.jsonl', sub_run_id: 'sub_TEST',
    });
    const parsed = parseSubrunOutput(json);
    expect(parsed).not.toBeNull();
    expect(parsed?.top).toEqual({
      ok: true,
      tokensUsed: 1234,
      transcriptPath: '/tmp/sub_TEST/transcript.jsonl',
      subRunID: 'sub_TEST',
      error: null,
    });
    expect(parsed?.requests).toEqual([]);
  });

  it('parses the parallel shape into per-request payloads', () => {
    const json = JSON.stringify({
      ok: true,
      results: {
        a: { ok: true, agent: 'a1', tokens_used: 10, transcript_path: '/t/a.jsonl', sub_run_id: 'sub_a' },
        b: { ok: false, agent: 'b1', error: 'boom' },
      },
      all_succeeded: true,
    });
    const parsed = parseSubrunOutput(json);
    expect(parsed?.top?.ok).toBe(true);
    expect(parsed?.requests.map((r) => r.id)).toEqual(['a', 'b']);
    expect(parsed?.requests[0].payload.tokensUsed).toBe(10);
    expect(parsed?.requests[0].payload.transcriptPath).toBe('/t/a.jsonl');
    expect(parsed?.requests[1].payload.ok).toBe(false);
    expect(parsed?.requests[1].payload.error).toBe('boom');
  });

  it('returns null for non-JSON, non-object JSON, and objects with none of the wire fields', () => {
    expect(parseSubrunOutput(undefined)).toBeNull();
    expect(parseSubrunOutput('nope')).toBeNull();
    expect(parseSubrunOutput('[1,2]')).toBeNull();
    expect(parseSubrunOutput('{"status": "completed", "total_tokens": 5}')).toBeNull();
  });
});

it('read ToolView renders output in a pre block with path in header', () => {
  const tv: ToolView = {
    tool: 'read_file',
    kind: 'read',
    input: { path: 'src/main.rs' },
    output: 'fn main() {\n  println!("hi");\n}',
  };
  render(<ToolCard tool={tv} />);
  // Header shows path
  expect(screen.getByText('src/main.rs')).not.toBeNull();
  // Output is in the pre block
  expect(screen.getByText(/fn main/)).not.toBeNull();
});

it('grep ToolView renders match count and lines', () => {
  const tv: ToolView = {
    tool: 'grep',
    kind: 'grep',
    input: { pattern: 'ToolView', path: 'src/' },
    output: 'src/a.ts:10:export type ToolView\nsrc/b.ts:5:import type { ToolView }',
  };
  render(<ToolCard tool={tv} />);
  expect(screen.getByText(/2 matches/)).not.toBeNull();
});

// ---------------------------------------------------------------------------
// 3. parseAstGrepText — pure fallback-parser unit tests
// ---------------------------------------------------------------------------

describe('parseAstGrepText', () => {
  it('empty input returns []', () => {
    expect(parseAstGrepText('')).toEqual([]);
  });

  it('whitespace-only input returns []', () => {
    expect(parseAstGrepText('   \n  \n')).toEqual([]);
  });

  it('groups matches from two files', () => {
    const output = [
      'src/a.rs:10:3: fn foo() {}',
      'src/a.rs:22:1: fn bar() {}',
      'src/b.rs:5:8: fn baz() {}',
    ].join('\n');
    expect(parseAstGrepText(output)).toEqual([
      {
        file: 'src/a.rs',
        matches: [
          { line: 10, col: 3, text: 'fn foo() {}' },
          { line: 22, col: 1, text: 'fn bar() {}' },
        ],
      },
      {
        file: 'src/b.rs',
        matches: [{ line: 5, col: 8, text: 'fn baz() {}' }],
      },
    ]);
  });

  it('skips a non-matching line without throwing', () => {
    const output = ['not a match line', 'src/a.rs:1:1: fn foo() {}'].join('\n');
    expect(parseAstGrepText(output)).toEqual([
      { file: 'src/a.rs', matches: [{ line: 1, col: 1, text: 'fn foo() {}' }] },
    ]);
  });
});

// ---------------------------------------------------------------------------
// 4. AstGrepBody — render smoke tests (via ToolCard dispatch)
// ---------------------------------------------------------------------------

describe('AstGrepBody (via ToolCard)', () => {
  it('renders structured payload: count badge, group-by-file, metavar highlight + bindings table', () => {
    const tv: ToolView = {
      tool: 'ast_grep',
      kind: 'ast_grep',
      input: { pattern: 'fn $NAME() {}', lang: 'rust' },
      output: 'src/a.rs:10:3: fn foo() {}',
      structured: {
        tool: 'ast_grep',
        pattern: 'fn $NAME() {}',
        lang: 'rust',
        matchCount: 1,
        fileCount: 1,
        truncated: false,
        matches: [
          {
            file: 'src/a.rs',
            range: { startLine: 10, startCol: 3, endLine: 10, endCol: 15 },
            text: 'fn foo() {}',
            metaVars: {
              single: { NAME: { text: 'foo', textOffset: { start: 3, end: 6 } } },
              multi: {},
            },
          },
        ],
      },
    };
    render(<ToolCard tool={tv} />);
    expect(screen.getByText(/1 match in 1 file/)).not.toBeNull();
    expect(screen.getByText('src/a.rs')).not.toBeNull();
    // Highlighted metavar span carries the binding's bound text and a $NAME title.
    const highlighted = screen.getByTitle('$NAME');
    expect(highlighted.textContent).toBe('foo');
    // Bindings table renders "$NAME" and its bound text.
    expect(screen.getByText('$NAME')).not.toBeNull();
  });

  it('renders truncated notice when structured.truncated is true', () => {
    const tv: ToolView = {
      tool: 'ast_grep',
      kind: 'ast_grep',
      input: { pattern: 'fn $NAME() {}', lang: 'rust' },
      structured: {
        matchCount: 500,
        fileCount: 40,
        truncated: true,
        matches: [
          { file: 'src/a.rs', range: { startLine: 1, startCol: 1, endLine: 1, endCol: 5 }, text: 'fn a() {}' },
        ],
      },
    };
    render(<ToolCard tool={tv} />);
    expect(screen.getByText(/showing first 1 of 500/)).not.toBeNull();
  });

  it('falls back to parseAstGrepText grouping when structured is absent', () => {
    const tv: ToolView = {
      tool: 'ast_grep',
      kind: 'ast_grep',
      input: { pattern: 'fn $NAME() {}', lang: 'rust' },
      output: 'src/a.rs:10:3: fn foo() {}\nsrc/b.rs:1:1: fn bar() {}',
    };
    render(<ToolCard tool={tv} />);
    expect(screen.getByText(/2 matches in 2 files/)).not.toBeNull();
    expect(screen.getByText('src/a.rs')).not.toBeNull();
    expect(screen.getByText('src/b.rs')).not.toBeNull();
  });

  it('error ToolView suppresses the body (error block handles it)', () => {
    const tv: ToolView = {
      tool: 'ast_grep',
      kind: 'ast_grep',
      input: { pattern: 'fn $NAME() {}' },
      error: 'ast-grep binary not found',
    };
    render(<ToolCard tool={tv} />);
    expect(screen.getByText('Error')).not.toBeNull();
    expect(screen.queryByText(/match/)).toBeNull();
  });

  // -------------------------------------------------------------------------
  // 5. Match header -> SourcePreview wiring
  // -------------------------------------------------------------------------

  const STRUCTURED_TV: ToolView = {
    tool: 'ast_grep',
    kind: 'ast_grep',
    input: { pattern: 'fn $NAME() {}', lang: 'rust' },
    structured: {
      tool: 'ast_grep',
      pattern: 'fn $NAME() {}',
      lang: 'rust',
      matchCount: 1,
      fileCount: 1,
      truncated: false,
      matches: [
        {
          file: 'src/a.rs',
          range: { startLine: 10, startCol: 3, endLine: 10, endCol: 15 },
          text: 'fn foo() {}',
        },
      ],
    },
  };

  const SLICE: SourceSlice = {
    available: true,
    path: 'src/a.rs',
    language: 'rust',
    startLine: 5,
    endLine: 15,
    targetLine: 10,
    totalLines: 100,
    lines: [{ n: 10, text: 'fn foo() {}' }],
  };

  it('clicking a structured match header mounts SourcePreview and calls api.readSource with file+line, when runId is provided', async () => {
    const spy = vi.spyOn(api, 'readSource').mockResolvedValue(SLICE);
    const { container } = render(<ToolCard tool={STRUCTURED_TV} runId="run-1" />);

    const header = screen.getByRole('button', { name: /src\/a\.rs:10:3/ });
    header.click();

    await waitFor(() =>
      expect(spy).toHaveBeenCalledWith('run-1', 'src/a.rs', 10, { host: undefined }),
    );
    // Mounted SourcePreview eventually renders its line-numbered slice with
    // the target line emphasized (a marker only SourcePreview produces).
    await waitFor(() =>
      expect(container.querySelector('[data-target="true"]')).not.toBeNull(),
    );
  });

  it('structured match header is non-clickable plain text when runId is absent', () => {
    render(<ToolCard tool={STRUCTURED_TV} />);
    expect(screen.queryByRole('button', { name: /src\/a\.rs:10:3/ })).toBeNull();
    expect(screen.getByText(/src\/a\.rs:10:3/)).not.toBeNull();
  });

  it('fallback (text-parsed) match header is clickable and mounts SourcePreview when runId is provided', async () => {
    const spy = vi.spyOn(api, 'readSource').mockResolvedValue(SLICE);
    const tv: ToolView = {
      tool: 'ast_grep',
      kind: 'ast_grep',
      input: { pattern: 'fn $NAME() {}' },
      output: 'src/a.rs:10:3: fn foo() {}',
    };
    render(<ToolCard tool={tv} runId="run-1" />);

    const header = screen.getByRole('button', { name: /src\/a\.rs:10:3:/ });
    header.click();

    await waitFor(() =>
      expect(spy).toHaveBeenCalledWith('run-1', 'src/a.rs', 10, { host: undefined }),
    );
  });

  // -------------------------------------------------------------------------
  // 6. Match header -> AstTree ("tree") wiring
  // -------------------------------------------------------------------------

  const AST_RESPONSE: AstResponse = {
    available: true,
    language: 'rust',
    truncated: false,
    root: {
      kind: 'function_item',
      named: true,
      startLine: 10,
      startCol: 3,
      endLine: 10,
      endCol: 15,
      matched: true,
      children: [],
    },
  };

  it('clicking the tree button on a structured match mounts AstTree and calls api.readAst with file+line+col, when runId is provided', async () => {
    const spy = vi.spyOn(api, 'readAst').mockResolvedValue(AST_RESPONSE);
    render(<ToolCard tool={STRUCTURED_TV} runId="run-1" />);

    const treeButton = screen.getByRole('button', { name: /^tree$/ });
    treeButton.click();

    await waitFor(() =>
      expect(spy).toHaveBeenCalledWith('run-1', 'src/a.rs', 10, 3, { host: undefined }),
    );
    // AstTree eventually renders the fetched root node's kind.
    await waitFor(() => expect(screen.getByText('function_item')).not.toBeNull());
  });

  it('no tree button on a structured match when runId is absent', () => {
    render(<ToolCard tool={STRUCTURED_TV} />);
    expect(screen.queryByRole('button', { name: /^tree$/ })).toBeNull();
  });

  it('source-preview and tree toggles are independent — both can be open at once', async () => {
    vi.spyOn(api, 'readSource').mockResolvedValue(SLICE);
    vi.spyOn(api, 'readAst').mockResolvedValue(AST_RESPONSE);
    const { container } = render(<ToolCard tool={STRUCTURED_TV} runId="run-1" />);

    screen.getByRole('button', { name: /src\/a\.rs:10:3/ }).click();
    screen.getByRole('button', { name: /^tree$/ }).click();

    await waitFor(() =>
      expect(container.querySelector('[data-target="true"]')).not.toBeNull(),
    );
    await waitFor(() => expect(screen.getByText('function_item')).not.toBeNull());
  });

  it('clicking the tree button on a fallback (text-parsed) match calls api.readAst with file+line+col', async () => {
    const spy = vi.spyOn(api, 'readAst').mockResolvedValue(AST_RESPONSE);
    const tv: ToolView = {
      tool: 'ast_grep',
      kind: 'ast_grep',
      input: { pattern: 'fn $NAME() {}' },
      output: 'src/a.rs:10:3: fn foo() {}',
    };
    render(<ToolCard tool={tv} runId="run-1" />);

    screen.getByRole('button', { name: /^tree$/ }).click();

    await waitFor(() =>
      expect(spy).toHaveBeenCalledWith('run-1', 'src/a.rs', 10, 3, { host: undefined }),
    );
  });

  it('no tree button on a fallback (text-parsed) match when runId is absent', () => {
    const tv: ToolView = {
      tool: 'ast_grep',
      kind: 'ast_grep',
      input: { pattern: 'fn $NAME() {}' },
      output: 'src/a.rs:10:3: fn foo() {}',
    };
    render(<ToolCard tool={tv} />);
    expect(screen.queryByRole('button', { name: /^tree$/ })).toBeNull();
  });
});
