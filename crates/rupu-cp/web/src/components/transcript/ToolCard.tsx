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

import type { ToolView, FindingView } from './transcriptView';
import FindingCard from './FindingCard';
import DiffView from './DiffView';
import TerminalBlock from './TerminalBlock';
import StructuredView from './StructuredView';

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
// Card header
// ---------------------------------------------------------------------------

function DurationBadge({ ms }: { ms: number }) {
  const text = ms < 1000 ? `${ms}ms` : `${(ms / 1000).toFixed(1)}s`;
  return (
    <span className="inline-flex items-center rounded px-1.5 py-0.5 text-[10px] font-mono bg-slate-100 text-slate-500">
      {text}
    </span>
  );
}

function StatusBadge({ tool }: { tool: ToolView }) {
  if (tool.error) {
    return (
      <span className="inline-flex items-center rounded px-1.5 py-0.5 text-[10px] font-semibold bg-red-100 text-red-700 ring-1 ring-inset ring-red-200">
        error
      </span>
    );
  }
  if (tool.durationMs !== undefined) {
    return <DurationBadge ms={tool.durationMs} />;
  }
  return (
    <span className="inline-flex items-center rounded px-1.5 py-0.5 text-[10px] bg-slate-100 text-slate-400">
      ok
    </span>
  );
}

function CardHeader({ tool, summary }: { tool: ToolView; summary: string }) {
  return (
    <div className="flex items-center gap-2 min-w-0 px-3 py-1.5 bg-slate-50 border-b border-slate-200">
      {/* Tool name */}
      <span className="font-mono text-[11px] font-semibold text-brand-700 shrink-0">
        ⚙ {tool.tool}
      </span>

      {/* Input summary */}
      {summary && (
        <span className="font-mono text-[10.5px] text-slate-500 truncate flex-1 min-w-0">
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
    <div className="bg-red-50 border-t border-red-200 px-3 py-2">
      <p className="text-[11px] font-semibold text-red-700 mb-0.5">Error</p>
      <pre className="whitespace-pre-wrap font-mono text-[10.5px] text-red-800 leading-snug">
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
      <pre className="whitespace-pre-wrap font-mono text-[10.5px] text-slate-700 bg-slate-50 rounded p-2 max-h-96 overflow-y-auto leading-snug">
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
        <p className="text-[10px] text-slate-400 mb-1">{lines.length} match{lines.length !== 1 ? 'es' : ''}</p>
      )}
      <div className="font-mono text-[10.5px] text-slate-700 bg-slate-50 rounded p-2 max-h-72 overflow-y-auto leading-snug space-y-0">
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
      <div className="font-mono text-[10.5px] text-slate-700 bg-slate-50 rounded p-2 max-h-64 overflow-y-auto leading-snug space-y-0">
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
            <span className="inline-flex items-center rounded px-2 py-0.5 text-[10px] bg-slate-100 text-slate-600 font-mono">
              status: {status}
            </span>
          )}
          {totalTokens !== null && (
            <span className="inline-flex items-center rounded px-2 py-0.5 text-[10px] bg-slate-100 text-slate-600 font-mono">
              {totalTokens.toLocaleString()} tokens
            </span>
          )}
        </div>

        {/* Transcript path / button */}
        {transcriptPath && (
          <div className="flex items-center gap-2">
            {onOpenTranscript ? (
              <button
                type="button"
                onClick={() => onOpenTranscript(transcriptPath)}
                className="inline-flex items-center rounded bg-brand-700 px-2.5 py-1 text-[11px] text-white font-medium hover:bg-brand-600 transition-colors"
              >
                View sub-run transcript →
              </button>
            ) : (
              <span className="inline-block font-mono text-[10.5px] text-brand-700 bg-slate-50 border border-border rounded px-2 py-0.5 break-all">
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
        <pre className="whitespace-pre-wrap font-mono text-[10.5px] text-slate-700 bg-slate-50 rounded p-2 max-h-64 overflow-y-auto">
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
        <pre className="whitespace-pre-wrap font-mono text-[10.5px] text-slate-700 bg-slate-50 rounded p-2 max-h-64 overflow-y-auto">
          {tool.output}
        </pre>
      ) : null}

      {/* Input args — only when non-trivial */}
      {showInput && (
        <div>
          <p className="text-[10px] uppercase tracking-wider text-slate-400 font-semibold mb-1">
            args
          </p>
          <StructuredView value={tool.input} />
        </div>
      )}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Public component
// ---------------------------------------------------------------------------

export default function ToolCard({
  tool,
  onOpenTranscript,
}: {
  tool: ToolView;
  onOpenTranscript?: (path: string) => void;
}) {
  // Findings get their own full chrome — no outer header wrapper.
  if (tool.kind === 'finding') {
    const finding = tool.finding as FindingView;
    return <FindingCard finding={finding} />;
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
    <div className="border border-slate-200 rounded-md overflow-hidden my-1 text-[11.5px]">
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
      {tool.kind === 'generic' && <GenericBody tool={tool} />}

      {/* Error block — shown when tool.error is set */}
      {tool.error && <ErrorBlock error={tool.error} />}
    </div>
  );
}
