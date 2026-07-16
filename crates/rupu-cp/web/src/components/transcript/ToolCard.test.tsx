// @vitest-environment jsdom
/**
 * Tests for ToolCard:
 *   1. summarizeInput helper — pure, no DOM (required)
 *   2. Smoke renders via testing-library (optional, per spec)
 */

import { it, expect, describe, afterEach, vi } from 'vitest';
import { render, screen, cleanup, waitFor } from '@testing-library/react';
import { summarizeInput, parseAstGrepText } from './ToolCard';
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
