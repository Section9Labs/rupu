/**
 * ToolCard — dispatcher that renders one card per ToolView, switching on
 * `tool.kind` to the appropriate bespoke sub-component.
 *
 * Anatomy:
 *   • `finding`  → FindingCard (its own full chrome; no outer header)
 *   • `diff`     → header + DiffView
 *   • `terminal` → header + TerminalBlock
 *   • `read`     → header + monospace pre block
 *   • `grep`     → header + mono match list
 *   • `glob`     → header + mono path list
 *   • `subrun`   → header + sub-run callout
 *   • `coverage` → header + StructuredView of parsed output / input
 *   • `ast_grep` → header + AstGrepBody (group-by-file, metavar highlight +
 *                  bindings table from `structured`, else a text-parse
 *                  fallback via `parseAstGrepText`)
 *   • `generic`  → header + StructuredView (JSON) or pre (plain) + input args
 *
 * Error state: when `tool.error` is set a red-tinted block is shown instead
 * of (or alongside) the body.
 *
 * Helper `summarizeInput` (exported) derives the short header summary string
 * from the tool input; unit-tested separately without DOM.
 *
 * No `any`. Static Tailwind class strings only.
 */

import { useState } from 'react';
import type { ReactNode } from 'react';
import { ChevronRight, ChevronDown } from 'lucide-react';
import type { ToolView, FindingView } from './transcriptView';
import FindingCard from './FindingCard';
import SourcePreview from './SourcePreview';
import AstTree from './AstTree';
import DiffView from './DiffView';
import TerminalBlock from './TerminalBlock';
import StructuredView from './StructuredView';
import { Button } from '../ui/Button';
import { Badge } from '../ui/Badge';
import { formatDuration } from '../../lib/duration';

// ---------------------------------------------------------------------------
// Public helper — exported for unit tests
// ---------------------------------------------------------------------------

/**
 * Derive a short (≤ ~60 char) summary string for the card header.
 * Returns an empty string when nothing useful can be extracted.
 */
export function summarizeInput(tool: ToolView): string {
  const inp = tool.input;
  if (inp === null || inp === undefined) return '';

  if (typeof inp === 'object' && !Array.isArray(inp)) {
    const rec = inp as Record<string, unknown>;

    switch (tool.kind) {
      case 'read': {
        const path = typeof rec.path === 'string' ? rec.path : '';
        if (!path) return '';
        const start = typeof rec.start_line === 'number' ? rec.start_line : null;
        const end   = typeof rec.end_line   === 'number' ? rec.end_line   : null;
        if (start !== null && end !== null) return `${path}:${start}-${end}`;
        if (start !== null) return `${path}:${start}`;
        return path;
      }

      case 'grep': {
        const pattern = typeof rec.pattern === 'string' ? rec.pattern : '';
        const path    = typeof rec.path    === 'string' ? rec.path    : '';
        if (pattern && path) return `${pattern}  ${path}`;
        return pattern || path;
      }

      case 'glob': {
        const pattern = typeof rec.pattern === 'string' ? rec.pattern : '';
        const path    = typeof rec.path    === 'string' ? rec.path    : '';
        return pattern || path;
      }

      case 'terminal': {
        // For bash calls the command may be in tool.terminal (paired event) or
        // in input.command / input.cmd.
        const cmd =
          typeof rec.command === 'string' ? rec.command :
          typeof rec.cmd     === 'string' ? rec.cmd     : '';
        return cmd.length > 60 ? cmd.slice(0, 57) + '…' : cmd;
      }

      case 'diff': {
        const path = typeof rec.path === 'string' ? rec.path : '';
        return path;
      }

      case 'ast_grep': {
        const pattern = typeof rec.pattern === 'string' ? rec.pattern : '';
        const lang    = typeof rec.lang    === 'string' ? rec.lang    : '';
        if (pattern && lang) return `${pattern} · ${lang}`;
        return pattern || lang;
      }

      default: {
        // For generic/coverage/subrun/finding: try a single meaningful string
        // key in priority order.
        for (const key of ['path', 'pattern', 'query', 'name', 'description']) {
          const v = rec[key];
          if (typeof v === 'string' && v.length > 0) {
            return v.length > 60 ? v.slice(0, 57) + '…' : v;
          }
        }
        return '';
      }
    }
  }

  if (typeof inp === 'string') {
    return inp.length > 60 ? inp.slice(0, 57) + '…' : inp;
  }

  return '';
}

// ---------------------------------------------------------------------------
// Small internal helpers
// ---------------------------------------------------------------------------

/**
 * Try JSON.parse; return the parsed value or null on failure.
 */
function tryParseJson(text: string | undefined): unknown | null {
  if (!text) return null;
  try {
    return JSON.parse(text) as unknown;
  } catch {
    return null;
  }
}

/** True when the value is a non-null, non-array object. */
function isRecord(v: unknown): v is Record<string, unknown> {
  return v !== null && typeof v === 'object' && !Array.isArray(v);
}

/** True when the value is a non-trivial object (has at least one key). */
function isNonTrivialObject(v: unknown): v is Record<string, unknown> {
  return isRecord(v) && Object.keys(v as Record<string, unknown>).length > 0;
}

// ---------------------------------------------------------------------------
// ast_grep — structured payload types + defensive parser
// ---------------------------------------------------------------------------

interface AstGrepRange {
  startLine: number;
  startCol: number;
  endLine: number;
  endCol: number;
}

/** A single metavar binding: the bound text, plus a char-offset span into the
 * match's `text` field (0-based, Unicode-scalar) when computable. */
interface AstGrepBinding {
  text: string;
  textOffset?: { start: number; end: number };
}

interface AstGrepMatch {
  file: string;
  range?: AstGrepRange;
  text?: string;
  metaVars?: {
    single?: Record<string, AstGrepBinding>;
    multi?: Record<string, AstGrepBinding[]>;
  };
}

interface AstGrepStructured {
  pattern?: string;
  lang?: string;
  matchCount: number;
  fileCount: number;
  truncated: boolean;
  matches: AstGrepMatch[];
}

/** Parse a single metavar binding `{ text, textOffset?: { start, end } }`. */
function asAstGrepBinding(v: unknown): AstGrepBinding | null {
  if (!isRecord(v) || typeof v.text !== 'string') return null;
  const binding: AstGrepBinding = { text: v.text };
  if (isRecord(v.textOffset) && typeof v.textOffset.start === 'number' && typeof v.textOffset.end === 'number') {
    binding.textOffset = { start: v.textOffset.start, end: v.textOffset.end };
  }
  return binding;
}

/** Parse one match: `{ file, range?, text?, metaVars?: { single?, multi? } }`. */
function asAstGrepMatch(v: unknown): AstGrepMatch | null {
  if (!isRecord(v) || typeof v.file !== 'string') return null;
  const match: AstGrepMatch = { file: v.file };
  if (typeof v.text === 'string') match.text = v.text;

  if (isRecord(v.range)) {
    const r = v.range;
    if (
      typeof r.startLine === 'number' &&
      typeof r.startCol === 'number' &&
      typeof r.endLine === 'number' &&
      typeof r.endCol === 'number'
    ) {
      match.range = { startLine: r.startLine, startCol: r.startCol, endLine: r.endLine, endCol: r.endCol };
    }
  }

  if (isRecord(v.metaVars)) {
    const single: Record<string, AstGrepBinding> = {};
    if (isRecord(v.metaVars.single)) {
      for (const [name, b] of Object.entries(v.metaVars.single)) {
        const parsed = asAstGrepBinding(b);
        if (parsed) single[name] = parsed;
      }
    }
    const multi: Record<string, AstGrepBinding[]> = {};
    if (isRecord(v.metaVars.multi)) {
      for (const [name, arr] of Object.entries(v.metaVars.multi)) {
        if (Array.isArray(arr)) {
          const bindings = arr
            .map(asAstGrepBinding)
            .filter((b): b is AstGrepBinding => b !== null);
          multi[name] = bindings;
        }
      }
    }
    match.metaVars = { single, multi };
  }

  return match;
}

/** Parse `tool.structured` into an `AstGrepStructured`, or null when the
 * shape doesn't match (falls back to the text parser in that case). */
function asAstGrepStructured(v: unknown): AstGrepStructured | null {
  if (!isRecord(v) || !Array.isArray(v.matches)) return null;
  const matches = v.matches
    .map(asAstGrepMatch)
    .filter((m): m is AstGrepMatch => m !== null);
  return {
    pattern: typeof v.pattern === 'string' ? v.pattern : undefined,
    lang: typeof v.lang === 'string' ? v.lang : undefined,
    matchCount: typeof v.matchCount === 'number' ? v.matchCount : matches.length,
    fileCount:
      typeof v.fileCount === 'number' ? v.fileCount : new Set(matches.map((m) => m.file)).size,
    truncated: v.truncated === true,
    matches,
  };
}

/**
 * Fallback parser for the compact `ast_grep` text output, used when
 * `tool.structured` is absent (e.g. older runs). Parses `path:line:col: text`
 * lines into per-file groups. Pure and exported for unit testing.
 */
export function parseAstGrepText(
  output: string,
): { file: string; matches: { line: number; col: number; text: string }[] }[] {
  const byFile = new Map<string, { line: number; col: number; text: string }[]>();
  for (const raw of output.split('\n')) {
    if (!raw.trim()) continue;
    const m = raw.match(/^(.*?):(\d+):(\d+): (.*)$/);
    if (!m) continue;
    const [, file, line, col, text] = m;
    if (!byFile.has(file)) byFile.set(file, []);
    byFile.get(file)!.push({ line: Number(line), col: Number(col), text });
  }
  return [...byFile.entries()].map(([file, matches]) => ({ file, matches }));
}

// ---------------------------------------------------------------------------
// Card header
// ---------------------------------------------------------------------------

function DurationBadge({ ms }: { ms: number }) {
  return (
    <span className="inline-flex items-center rounded px-1.5 py-0.5 text-meta font-mono bg-surface text-ink-dim">
      {formatDuration(ms)}
    </span>
  );
}

function StatusBadge({ tool }: { tool: ToolView }) {
  if (tool.error) {
    return (
      <span className="inline-flex items-center rounded px-1.5 py-0.5 text-meta font-semibold bg-err-bg text-err ring-1 ring-inset ring-err/30">
        error
      </span>
    );
  }
  if (tool.durationMs !== undefined) {
    return <DurationBadge ms={tool.durationMs} />;
  }
  return (
    <span className="inline-flex items-center rounded px-1.5 py-0.5 text-meta bg-surface text-ink-mute">
      ok
    </span>
  );
}

function CardHeader({ tool, summary }: { tool: ToolView; summary: string }) {
  return (
    <div className="flex items-center gap-2 min-w-0 px-3 py-1.5 bg-surface border-b border-border">
      {/* Tool name */}
      <span className="font-mono text-note font-semibold text-brand-700 shrink-0">
        ⚙ {tool.tool}
      </span>

      {/* Input summary */}
      {summary && (
        <span className="font-mono text-[10.5px] text-ink-dim truncate flex-1 min-w-0">
          {summary}
        </span>
      )}

      {/* Right-side badge */}
      <span className="shrink-0 ml-auto">
        <StatusBadge tool={tool} />
      </span>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Error block
// ---------------------------------------------------------------------------

function ErrorBlock({ error }: { error: string }) {
  return (
    <div className="bg-err-bg border-t border-err/30 px-3 py-2">
      <p className="text-note font-semibold text-err mb-0.5">Error</p>
      <pre className="whitespace-pre-wrap font-mono text-[10.5px] text-err leading-snug">
        {error}
      </pre>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Kind-specific bodies
// ---------------------------------------------------------------------------

/** read — monospace pre, max-height scroll */
function ReadBody({ tool }: { tool: ToolView }) {
  if (tool.error) return null; // error block handles it
  if (!tool.output) return null;
  return (
    <div className="px-3 py-2">
      <pre className="whitespace-pre-wrap font-mono text-[10.5px] text-ink bg-surface rounded p-2 max-h-96 overflow-y-auto leading-snug">
        {tool.output}
      </pre>
    </div>
  );
}

/** grep — split output into mono match lines, show match count in summary */
function GrepBody({ tool }: { tool: ToolView }) {
  if (tool.error) return null;
  if (!tool.output) return null;
  const lines = tool.output.split('\n').filter((l) => l.length > 0);
  return (
    <div className="px-3 py-2">
      {lines.length > 0 && (
        <p className="text-meta text-ink-mute mb-1">{lines.length} match{lines.length !== 1 ? 'es' : ''}</p>
      )}
      <div className="font-mono text-[10.5px] text-ink bg-surface rounded p-2 max-h-72 overflow-y-auto leading-snug space-y-0">
        {lines.map((line, i) => (
          <div key={i} className="whitespace-pre">{line}</div>
        ))}
      </div>
    </div>
  );
}

/** glob — split output into mono path lines */
function GlobBody({ tool }: { tool: ToolView }) {
  if (tool.error) return null;
  if (!tool.output) return null;
  const paths = tool.output.split('\n').filter((l) => l.length > 0);
  return (
    <div className="px-3 py-2">
      <div className="font-mono text-[10.5px] text-ink bg-surface rounded p-2 max-h-64 overflow-y-auto leading-snug space-y-0">
        {paths.map((p, i) => (
          <div key={i} className="whitespace-pre">{p}</div>
        ))}
      </div>
    </div>
  );
}

/** subrun — callout with transcript_path link / button */
function SubrunBody({
  tool,
  onOpenTranscript,
}: {
  tool: ToolView;
  onOpenTranscript?: (path: string) => void;
}) {
  if (tool.error) return null;

  const parsed = tryParseJson(tool.output);
  const rec = isRecord(parsed) ? parsed : null;
  const transcriptPath =
    rec && typeof rec.transcript_path === 'string' ? rec.transcript_path : null;

  // Sub-run metadata from parsed object
  const totalTokens =
    rec && typeof rec.total_tokens === 'number' ? rec.total_tokens : null;
  const status =
    rec && typeof rec.status === 'string' ? rec.status : null;

  if (rec) {
    return (
      <div className="px-3 py-2 space-y-2">
        {/* Summary chips */}
        <div className="flex flex-wrap gap-1.5 items-center">
          {status && (
            <Badge tone="neutral" className="px-2 font-mono">
              status: {status}
            </Badge>
          )}
          {totalTokens !== null && (
            <Badge tone="neutral" className="px-2 font-mono">
              {totalTokens.toLocaleString()} tokens
            </Badge>
          )}
        </div>

        {/* Transcript path / button */}
        {transcriptPath && (
          <div className="flex items-center gap-2">
            {onOpenTranscript ? (
              <Button
                variant="primary"
                size="sm"
                onClick={() => onOpenTranscript(transcriptPath)}
              >
                View sub-run transcript →
              </Button>
            ) : (
              <span className="inline-block font-mono text-[10.5px] text-brand-700 bg-surface border border-border rounded px-2 py-0.5 break-all">
                {transcriptPath}
              </span>
            )}
          </div>
        )}
      </div>
    );
  }

  // Parse failed — fall back to StructuredView / pre
  if (tool.output) {
    return (
      <div className="px-3 py-2">
        <pre className="whitespace-pre-wrap font-mono text-[10.5px] text-ink bg-surface rounded p-2 max-h-64 overflow-y-auto">
          {tool.output}
        </pre>
      </div>
    );
  }

  return null;
}

/** coverage — StructuredView of parsed output JSON, or input as fallback */
function CoverageBody({ tool }: { tool: ToolView }) {
  if (tool.error) return null;
  const parsed = tryParseJson(tool.output);
  const value = parsed !== null ? parsed : tool.input;
  return (
    <div className="px-3 py-2">
      <StructuredView value={value} />
    </div>
  );
}

/** generic — StructuredView or pre for output; small "args" section for input */
function GenericBody({ tool }: { tool: ToolView }) {
  if (tool.error) return null;

  const parsedOutput = tryParseJson(tool.output);
  const showInput = isNonTrivialObject(tool.input);

  return (
    <div className="px-3 py-2 space-y-2">
      {/* Output */}
      {parsedOutput !== null ? (
        <StructuredView value={parsedOutput} />
      ) : tool.output ? (
        <pre className="whitespace-pre-wrap font-mono text-[10.5px] text-ink bg-surface rounded p-2 max-h-64 overflow-y-auto">
          {tool.output}
        </pre>
      ) : null}

      {/* Input args — only when non-trivial */}
      {showInput && (
        <div>
          <p className="text-meta uppercase tracking-wider text-ink-mute font-semibold mb-1">
            args
          </p>
          <StructuredView value={tool.input} />
        </div>
      )}
    </div>
  );
}

/** Collapsible per-file group — chevron + file path + match-count badge,
 * mirroring the disclosure pattern in `Turn.tsx`. Defaults open since the
 * matches are the point of an ast_grep call. */
function FileGroup({
  file,
  count,
  children,
}: {
  file: string;
  count: number;
  children: ReactNode;
}) {
  const [open, setOpen] = useState(true);
  return (
    <div className="mb-1 rounded border border-border overflow-hidden">
      <button
        type="button"
        onClick={() => setOpen((v) => !v)}
        className="flex w-full items-center gap-1.5 px-2 py-1 text-left bg-surface"
      >
        {open ? (
          <ChevronDown size={11} className="shrink-0 text-ink-mute" />
        ) : (
          <ChevronRight size={11} className="shrink-0 text-ink-mute" />
        )}
        <span className="min-w-0 flex-1 truncate font-mono text-[10.5px] text-ink">{file}</span>
        <span className="shrink-0 text-meta text-ink-mute">
          {count} match{count === 1 ? '' : 'es'}
        </span>
      </button>
      {open && <div className="space-y-1 px-2 py-1">{children}</div>}
    </div>
  );
}

/** Collect [start,end,name] spans from single + multi metavar bindings,
 * sorted by start offset (ready for sequential slicing). */
function collectMetaVarSpans(
  single: Record<string, AstGrepBinding> | undefined,
  multi: Record<string, AstGrepBinding[]> | undefined,
): { start: number; end: number; name: string }[] {
  const spans: { start: number; end: number; name: string }[] = [];
  for (const [name, b] of Object.entries(single ?? {})) {
    if (b.textOffset) spans.push({ start: b.textOffset.start, end: b.textOffset.end, name });
  }
  for (const [name, arr] of Object.entries(multi ?? {})) {
    for (const b of arr) {
      if (b.textOffset) spans.push({ start: b.textOffset.start, end: b.textOffset.end, name });
    }
  }
  return spans.sort((a, z) => a.start - z.start);
}

/** Renders a match's snippet with metavar bindings highlighted inline.
 * Slices with `Array.from(text)` (a codepoint array) so indices align with
 * the Rust side's char (Unicode-scalar) offsets rather than UTF-16 units. */
function HighlightedMatch({
  text,
  single,
  multi,
}: {
  text: string;
  single?: Record<string, AstGrepBinding>;
  multi?: Record<string, AstGrepBinding[]>;
}) {
  const chars = Array.from(text);
  const spans = collectMetaVarSpans(single, multi);
  const out: ReactNode[] = [];
  let cursor = 0;
  spans.forEach((s, i) => {
    // Skip overlapping or out-of-range spans rather than corrupting the render.
    if (s.start < cursor || s.end < s.start || s.end > chars.length) return;
    if (s.start > cursor) out.push(<span key={`t${i}`}>{chars.slice(cursor, s.start).join('')}</span>);
    out.push(
      <span
        key={`m${i}`}
        className="rounded bg-warn-bg text-warn px-0.5"
        title={`$${s.name}`}
      >
        {chars.slice(s.start, s.end).join('')}
      </span>,
    );
    cursor = s.end;
  });
  if (cursor < chars.length) out.push(<span key="tail">{chars.slice(cursor).join('')}</span>);
  return <code className="whitespace-pre-wrap">{out}</code>;
}

/** `$name = text` bindings table — single vars first, then multi (one row
 * per captured element, indexed). Renders nothing when both maps are empty. */
function MetaVarTable({
  single,
  multi,
}: {
  single?: Record<string, AstGrepBinding>;
  multi?: Record<string, AstGrepBinding[]>;
}) {
  const singleEntries = Object.entries(single ?? {});
  const multiEntries = Object.entries(multi ?? {});
  if (singleEntries.length === 0 && multiEntries.length === 0) return null;
  return (
    <table className="mt-1 font-mono text-[10.5px] leading-snug">
      <tbody>
        {singleEntries.map(([name, b]) => (
          <tr key={name}>
            <td className="pr-2 align-top text-brand-700">${name}</td>
            <td className="align-top text-ink-dim">{b.text}</td>
          </tr>
        ))}
        {multiEntries.map(([name, arr]) =>
          arr.map((b, i) => (
            <tr key={`${name}-${i}`}>
              <td className="pr-2 align-top text-brand-700">
                ${name}[{i}]
              </td>
              <td className="align-top text-ink-dim">{b.text}</td>
            </tr>
          )),
        )}
      </tbody>
    </table>
  );
}

/**
 * One structured ast_grep match: its `file:line:col` header (a toggle button
 * when `runId` is known, else plain text), a sibling "tree" button, the
 * highlighted snippet + metavar table, and — when toggled open — an inline
 * `SourcePreview` and/or `AstTree` of the match's location. The source-preview
 * and syntax-tree toggles use independent `useState`s so either, both, or
 * neither can be open at once.
 */
function AstGrepMatchRow({
  file,
  match,
  runId,
  host,
}: {
  file: string;
  match: AstGrepMatch;
  runId?: string;
  host?: string;
}) {
  const [open, setOpen] = useState(false);
  const [treeOpen, setTreeOpen] = useState(false);
  const range = match.range;

  return (
    <div className="border-l-2 border-border pl-2 py-1">
      {range && (
        <div className="flex items-center gap-2">
          {runId ? (
            <button
              type="button"
              onClick={() => setOpen((v) => !v)}
              className="text-meta font-mono text-ink-mute hover:text-brand-700 hover:underline"
            >
              {file}:{range.startLine}:{range.startCol}
            </button>
          ) : (
            <div className="text-meta text-ink-mute">
              {file}:{range.startLine}:{range.startCol}
            </div>
          )}
          {runId && (
            <button
              type="button"
              onClick={() => setTreeOpen((v) => !v)}
              className="text-meta font-mono text-ink-mute hover:text-brand-700 hover:underline"
            >
              tree
            </button>
          )}
        </div>
      )}
      <div className="font-mono text-[10.5px] text-ink">
        <HighlightedMatch
          text={match.text ?? ''}
          single={match.metaVars?.single}
          multi={match.metaVars?.multi}
        />
      </div>
      <MetaVarTable single={match.metaVars?.single} multi={match.metaVars?.multi} />
      {open && range && runId && (
        <SourcePreview runId={runId} path={file} line={range.startLine} host={host} />
      )}
      {treeOpen && range && runId && (
        <AstTree runId={runId} path={file} line={range.startLine} col={range.startCol} host={host} />
      )}
    </div>
  );
}

/** One fallback (text-parsed) ast_grep match: `file:line:col:` header (a
 * toggle button when `runId` is known, else plain text) + the raw match
 * text + a sibling "tree" button, with an inline `SourcePreview` and/or
 * `AstTree` when toggled open (independent `useState`s — either, both, or
 * neither can be open). Text-parsed matches always carry a `col` from the
 * `path:line:col:` regex, but `?? 1` guards the (defensive) case where it
 * doesn't. */
function AstGrepTextMatchRow({
  file,
  match,
  runId,
  host,
}: {
  file: string;
  match: { line: number; col: number; text: string };
  runId?: string;
  host?: string;
}) {
  const [open, setOpen] = useState(false);
  const [treeOpen, setTreeOpen] = useState(false);
  const col = match.col ?? 1;

  return (
    <div>
      <div className="whitespace-pre font-mono text-[10.5px] text-ink">
        {runId ? (
          <button
            type="button"
            onClick={() => setOpen((v) => !v)}
            className="text-ink-mute hover:text-brand-700 hover:underline"
          >
            {file}:{match.line}:{match.col}:{' '}
          </button>
        ) : (
          <span className="text-ink-mute">
            {file}:{match.line}:{match.col}:{' '}
          </span>
        )}
        {match.text}
        {runId && (
          <button
            type="button"
            onClick={() => setTreeOpen((v) => !v)}
            className="ml-2 text-meta font-mono text-ink-mute hover:text-brand-700 hover:underline"
          >
            tree
          </button>
        )}
      </div>
      {open && runId && (
        <SourcePreview runId={runId} path={file} line={match.line} host={host} />
      )}
      {treeOpen && runId && (
        <AstTree runId={runId} path={file} line={match.line} col={col} host={host} />
      )}
    </div>
  );
}

/** ast_grep — structured render (group-by-file, count badge, metavar
 * highlight + bindings table) when `tool.structured` is present, else a
 * text-parse fallback via `parseAstGrepText`. Never a raw blob. Each match's
 * `file:line[:col]` header is clickable when `runId` is known (threaded from
 * `ToolCard`), toggling an inline `SourcePreview`. */
function AstGrepBody({ tool, runId, host }: { tool: ToolView; runId?: string; host?: string }) {
  if (tool.error) return null;

  const structured = asAstGrepStructured(tool.structured);

  if (structured) {
    const byFile = new Map<string, AstGrepMatch[]>();
    for (const m of structured.matches) {
      const list = byFile.get(m.file) ?? [];
      list.push(m);
      byFile.set(m.file, list);
    }
    return (
      <div className="px-3 py-2">
        <div className="mb-1.5 flex flex-wrap items-center gap-1.5 text-meta text-ink-mute">
          <span>
            {structured.matchCount} match{structured.matchCount === 1 ? '' : 'es'} in{' '}
            {structured.fileCount} file{structured.fileCount === 1 ? '' : 's'}
          </span>
          {structured.pattern && (
            <Badge tone="neutral" className="font-mono">
              {structured.pattern}
            </Badge>
          )}
          {structured.lang && (
            <Badge tone="neutral" className="font-mono">
              {structured.lang}
            </Badge>
          )}
          {structured.truncated && (
            <Badge tone="amber">
              showing first {structured.matches.length} of {structured.matchCount}
            </Badge>
          )}
        </div>
        {[...byFile.entries()].map(([file, ms]) => (
          <FileGroup key={file} file={file} count={ms.length}>
            {ms.map((m, i) => (
              <AstGrepMatchRow key={i} file={file} match={m} runId={runId} host={host} />
            ))}
          </FileGroup>
        ))}
      </div>
    );
  }

  // Fallback: parse the compact `path:line:col: text` text output.
  const groups = parseAstGrepText(tool.output ?? '');
  const count = groups.reduce((n, g) => n + g.matches.length, 0);
  return (
    <div className="px-3 py-2">
      <div className="mb-1.5 text-meta text-ink-mute">
        {count} match{count === 1 ? '' : 'es'} in {groups.length} file{groups.length === 1 ? '' : 's'}
      </div>
      {groups.map((g) => (
        <FileGroup key={g.file} file={g.file} count={g.matches.length}>
          {g.matches.map((m, i) => (
            <AstGrepTextMatchRow key={i} file={g.file} match={m} runId={runId} host={host} />
          ))}
        </FileGroup>
      ))}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Public component
// ---------------------------------------------------------------------------

export default function ToolCard({
  tool,
  onOpenTranscript,
  runId,
  host,
}: {
  tool: ToolView;
  onOpenTranscript?: (path: string) => void;
  /** Run id for the source-preview affordance (ast_grep match headers,
   *  finding location chips). Threaded down from `TranscriptPanel` via
   *  `Turn`. Absent → those references render as non-clickable text. */
  runId?: string;
  /** Remote host id to forward to `api.readSource`, mirroring the
   *  transcript fetch's `host` plumbing. */
  host?: string;
}) {
  // Findings get their own full chrome — no outer header wrapper.
  if (tool.kind === 'finding') {
    const finding = tool.finding as FindingView;
    return <FindingCard finding={finding} runId={runId} host={host} />;
  }

  const summary = summarizeInput(tool);

  // For terminal the TerminalBlock shows its own exit badge in the meta row —
  // we also want the command in the header (from terminal.command if available,
  // else from input).
  const terminalSummary =
    tool.kind === 'terminal' && tool.terminal
      ? tool.terminal.command.length > 60
        ? tool.terminal.command.slice(0, 57) + '…'
        : tool.terminal.command
      : summary;

  const headerSummary = tool.kind === 'terminal' ? terminalSummary : summary;

  return (
    <div className="border border-border rounded-md overflow-hidden my-1 text-[11.5px]">
      <CardHeader tool={tool} summary={headerSummary} />

      {/* Kind-specific body */}
      {tool.kind === 'diff' && tool.diff && (
        <DiffView
          diff={tool.diff.diff}
          path={tool.diff.path}
          editKind={tool.diff.editKind}
        />
      )}

      {tool.kind === 'terminal' && tool.terminal && (
        <TerminalBlock
          command={tool.terminal.command}
          output={tool.output}
          exitCode={tool.terminal.exitCode}
          cwd={tool.terminal.cwd}
        />
      )}

      {tool.kind === 'read' && <ReadBody tool={tool} />}
      {tool.kind === 'grep' && <GrepBody tool={tool} />}
      {tool.kind === 'glob' && <GlobBody tool={tool} />}
      {tool.kind === 'subrun' && (
        <SubrunBody tool={tool} onOpenTranscript={onOpenTranscript} />
      )}
      {tool.kind === 'coverage' && <CoverageBody tool={tool} />}
      {tool.kind === 'ast_grep' && <AstGrepBody tool={tool} runId={runId} host={host} />}
      {tool.kind === 'generic' && <GenericBody tool={tool} />}

      {/* Error block — shown when tool.error is set */}
      {tool.error && <ErrorBlock error={tool.error} />}
    </div>
  );
}
