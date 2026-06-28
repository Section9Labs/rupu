/**
 * StructuredView — dependency-free recursive key/value renderer for arbitrary
 * JSON values (tool inputs/outputs, API payloads, etc.).
 *
 * Accepts `value: unknown` and dispatches to the correct presentation:
 *   • object           → indented key/value rows
 *   • homogeneous array (all non-null objects) → compact <table>
 *   • scalar array (strings/numbers/bools)      → chip list
 *   • boolean          → colour pill
 *   • number           → mono span
 *   • string           → inline (short) or <pre> (long/multiline)
 *   • null / undefined → dim placeholder
 *   • depth > 4        → raw JSON.stringify fallback (prevents runaway recursion)
 *
 * No `any`.  Static Tailwind class strings only.
 */

// ---------------------------------------------------------------------------
// Type guards
// ---------------------------------------------------------------------------

function isRecord(v: unknown): v is Record<string, unknown> {
  return v !== null && typeof v === 'object' && !Array.isArray(v);
}

function isHomogeneousObjectArray(v: unknown[]): v is Record<string, unknown>[] {
  return v.length > 0 && v.every((el) => isRecord(el));
}

function isScalarArray(v: unknown[]): boolean {
  return v.every(
    (el) =>
      el === null ||
      typeof el === 'string' ||
      typeof el === 'number' ||
      typeof el === 'boolean',
  );
}

// Union of all unique keys across an array of records
function unionKeys(rows: Record<string, unknown>[]): string[] {
  const seen = new Set<string>();
  for (const row of rows) {
    for (const k of Object.keys(row)) seen.add(k);
  }
  return Array.from(seen);
}

// ---------------------------------------------------------------------------
// Sub-renderers (pure functional, no hooks)
// ---------------------------------------------------------------------------

const DEPTH_CAP = 4;
const STRING_INLINE_MAX = 120;

function ScalarChip({ v }: { v: string | number | null }) {
  const text = v === null ? 'null' : String(v);
  return (
    <span className="inline-block bg-surface text-ink rounded px-1.5 py-0.5 font-mono text-xs mr-1 mb-1">
      {text}
    </span>
  );
}

function TableView({
  rows,
  depth,
}: {
  rows: Record<string, unknown>[];
  depth: number;
}) {
  const keys = unionKeys(rows);
  return (
    <div className="overflow-x-auto my-1">
      <table className="text-xs border-collapse w-full">
        <thead>
          <tr>
            {keys.map((k) => (
              <th
                key={k}
                className="text-left font-mono text-brand-700 bg-surface border border-border px-2 py-0.5 whitespace-nowrap"
              >
                {k}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {rows.map((row, i) => (
            <tr key={i} className="even:bg-surface">
              {keys.map((k) => (
                <td
                  key={k}
                  className="border border-border px-2 py-0.5 align-top"
                >
                  <StructuredView value={row[k]} depth={depth + 1} />
                </td>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

function ObjectView({
  obj,
  depth,
}: {
  obj: Record<string, unknown>;
  depth: number;
}) {
  const entries = Object.entries(obj);
  if (entries.length === 0) {
    return <span className="font-mono text-xs text-ink-mute">{'{}'}</span>;
  }
  return (
    <div className="space-y-0.5">
      {entries.map(([k, v]) => (
        <div key={k} className="flex gap-2 items-start min-w-0">
          <span className="shrink-0 font-mono text-xs text-brand-700 pt-0.5 select-all">
            {k}
          </span>
          <div className="min-w-0 flex-1">
            <StructuredView value={v} depth={depth + 1} />
          </div>
        </div>
      ))}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Main component
// ---------------------------------------------------------------------------

export default function StructuredView({
  value,
  depth = 0,
}: {
  value: unknown;
  depth?: number;
}) {
  // Depth cap — prevent runaway recursion on deeply nested data
  if (depth > DEPTH_CAP) {
    return (
      <pre className="whitespace-pre-wrap font-mono text-xs text-ink bg-surface rounded p-1">
        {JSON.stringify(value, null, 2)}
      </pre>
    );
  }

  // null
  if (value === null) {
    return <span className="font-mono text-xs text-ink-mute">null</span>;
  }

  // undefined
  if (value === undefined) {
    return <span className="font-mono text-xs text-ink-mute">—</span>;
  }

  // boolean
  if (typeof value === 'boolean') {
    return value ? (
      <span className="inline-block rounded-full bg-ok-bg text-ok text-xs px-2 py-0.5 font-mono font-medium">
        true
      </span>
    ) : (
      <span className="inline-block rounded-full bg-surface text-ink-dim text-xs px-2 py-0.5 font-mono font-medium">
        false
      </span>
    );
  }

  // number
  if (typeof value === 'number') {
    return (
      <span className="font-mono text-xs text-ink">{String(value)}</span>
    );
  }

  // string
  if (typeof value === 'string') {
    const isLong = value.length > STRING_INLINE_MAX || value.includes('\n');
    if (isLong) {
      return (
        <pre className="whitespace-pre-wrap font-mono text-xs text-ink bg-surface rounded p-1 my-0.5">
          {value}
        </pre>
      );
    }
    return <span className="text-xs text-ink">{value}</span>;
  }

  // array
  if (Array.isArray(value)) {
    if (value.length === 0) {
      return <span className="font-mono text-xs text-ink-mute">[]</span>;
    }

    if (isHomogeneousObjectArray(value)) {
      return <TableView rows={value} depth={depth} />;
    }

    if (isScalarArray(value)) {
      return (
        <span className="flex flex-wrap">
          {value.map((el, i) => (
            <ScalarChip key={i} v={el as string | number | null} />
          ))}
        </span>
      );
    }

    // Mixed array — render as numbered rows
    return (
      <div className="space-y-0.5 pl-2 border-l border-border">
        {value.map((el, i) => (
          <div key={i} className="flex gap-2 items-start">
            <span className="shrink-0 font-mono text-xs text-ink-mute pt-0.5">
              {i}
            </span>
            <div className="min-w-0 flex-1">
              <StructuredView value={el} depth={depth + 1} />
            </div>
          </div>
        ))}
      </div>
    );
  }

  // object (non-null, non-array — already guarded above)
  if (isRecord(value)) {
    return <ObjectView obj={value} depth={depth} />;
  }

  // Fallback for anything else (shouldn't happen with well-typed JSON)
  return (
    <span className="font-mono text-xs text-ink">
      {String(value)}
    </span>
  );
}
