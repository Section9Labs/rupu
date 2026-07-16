// @vitest-environment jsdom
/**
 * Tests for ToolCard:
 *   1. summarizeInput helper — pure, no DOM (required)
 *   2. Smoke renders via testing-library (optional, per spec)
 */

import { it, expect, describe, afterEach } from 'vitest';
import { render, screen, cleanup } from '@testing-library/react';
import { summarizeInput } from './ToolCard';
import ToolCard from './ToolCard';
import type { ToolView, FindingView } from './transcriptView';

afterEach(cleanup);

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

it('subrun ToolView with transcript_path and callback renders button', () => {
  let clicked = '';
  const tv: ToolView = {
    tool: 'dispatch_agent',
    kind: 'subrun',
    input: { agent: 'scanner' },
    output: JSON.stringify({
      status: 'completed',
      total_tokens: 4200,
      transcript_path: '/runs/abc/transcript.jsonl',
    }),
  };
  render(<ToolCard tool={tv} onOpenTranscript={(p) => { clicked = p; }} />);
  const btn = screen.getByRole('button', { name: /View sub-run transcript/ });
  expect(btn).not.toBeNull();
  btn.click();
  expect(clicked).toBe('/runs/abc/transcript.jsonl');
});

it('subrun ToolView without callback renders path as chip (no button)', () => {
  const tv: ToolView = {
    tool: 'dispatch_agent',
    kind: 'subrun',
    input: { agent: 'scanner' },
    output: JSON.stringify({
      transcript_path: '/runs/abc/transcript.jsonl',
    }),
  };
  render(<ToolCard tool={tv} />);
  // No button
  expect(screen.queryByRole('button', { name: /View sub-run transcript/ })).toBeNull();
  // Path shown as chip text
  expect(screen.getByText('/runs/abc/transcript.jsonl')).not.toBeNull();
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
