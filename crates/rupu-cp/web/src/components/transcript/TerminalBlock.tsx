/**
 * TerminalBlock — dark terminal-style block for bash tool results.
 *
 * Anatomy (top → bottom):
 *   1. Prompt row   — "$ {command}" (emerald prompt, light command text)
 *   2. Output body  — pre-wrap mono text, scrollable max-height, slate-200
 *   3. Meta row     — exit-code badge (green 0 / red non-zero) + dim cwd
 *
 * Props: { command, output?, exitCode?, cwd? }
 * No `any`.  Static Tailwind class strings only.
 */

// ---------------------------------------------------------------------------
// Sub-components
// ---------------------------------------------------------------------------

function ExitBadge({ code }: { code: number }) {
  const isOk = code === 0;
  return (
    <span
      className={
        isOk
          ? 'inline-flex items-center rounded px-1.5 py-0.5 text-[10px] font-mono font-semibold bg-green-900/40 text-green-300'
          : 'inline-flex items-center rounded px-1.5 py-0.5 text-[10px] font-mono font-semibold bg-red-900/50 text-red-300'
      }
    >
      exit {code}
    </span>
  );
}

// ---------------------------------------------------------------------------
// Main component
// ---------------------------------------------------------------------------

export default function TerminalBlock({
  command,
  output,
  exitCode,
  cwd,
}: {
  command: string;
  output?: string;
  exitCode?: number;
  cwd?: string;
}) {
  const hasOutput = output !== undefined && output !== '';
  const hasMeta = exitCode !== undefined || Boolean(cwd);

  return (
    <div className="rounded-md bg-slate-900 text-slate-100 overflow-hidden my-1 text-[11.5px]">
      {/* 1. Prompt row */}
      <div className="flex items-start gap-1.5 px-3 pt-2.5 pb-2">
        <span className="shrink-0 text-emerald-400 font-mono font-bold select-none">$</span>
        <span className="font-mono text-slate-100 break-all">{command}</span>
      </div>

      {/* 2. Output body */}
      {hasOutput && (
        <div className="border-t border-slate-700/60 px-3 py-2">
          <pre className="whitespace-pre-wrap font-mono text-slate-200 max-h-96 overflow-y-auto leading-5 m-0">
            {output}
          </pre>
        </div>
      )}

      {/* 3. Meta row */}
      {hasMeta && (
        <div className="flex items-center gap-2 px-3 py-1.5 border-t border-slate-700/60">
          {exitCode !== undefined && <ExitBadge code={exitCode} />}
          {cwd && (
            <span className="font-mono text-[10px] text-slate-500 truncate">{cwd}</span>
          )}
        </div>
      )}
    </div>
  );
}
