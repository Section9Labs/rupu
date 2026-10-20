/**
 * AssetCard — bespoke view for `assets.mark` (engagement asset marking).
 *
 * `assets.mark` records how deeply an engagement asset has been examined, at a
 * rung of its profile's depth ladder. Its input is a typed locator — a `kind`
 * (e.g. `network:service`), a bag of `coordinates` (each `{t, v}`, see
 * `rupu-coverage`'s `Coordinate`), a `depth` rung and a free-text `label`.
 *
 * Two things this view does that the generic JSON fallback can't:
 *   1. renders the coordinates as legible chips — a host, a `443/tcp` port, a
 *      `10-20` line range — instead of nested `{t,v}` objects;
 *   2. turns the `assets.mark` serde "invalid input" errors (the agent sent the
 *      wrong shape — a port as a bare string/integer, a struct missing a field)
 *      into a plain-language "expected vs got" explanation, since that's an
 *      authoring mistake to fix, not a crash.
 *
 * The parsing (`parseAsset`) is pure and unit-tested without the DOM.
 * No `any`. Static Tailwind class strings only.
 */

import type { ToolView } from './transcriptView';
import { Chip } from './toolChrome';

// ---------------------------------------------------------------------------
// Coordinate model (mirrors rupu-coverage/src/asset/coordinate.rs)
// ---------------------------------------------------------------------------

function isRecord(v: unknown): v is Record<string, unknown> {
  return v !== null && typeof v === 'object' && !Array.isArray(v);
}

/** The value shape each coordinate tag expects, and a display formatter. */
interface CoordSpec {
  valid: (v: unknown) => boolean;
  format: (v: unknown) => string;
  /** Example of a correct `{t, v}` for the error hint (object-valued tags). */
  example?: string;
}

const STR: CoordSpec = { valid: (v) => typeof v === 'string', format: (v) => String(v) };
const NUM: CoordSpec = { valid: (v) => typeof v === 'number', format: (v) => String(v) };

const COORD_SPECS: Record<string, CoordSpec> = {
  path: STR,
  symbol: STR,
  commit: STR,
  sha256: STR,
  host: STR,
  url: STR,
  param: STR,
  offset: NUM,
  address: NUM,
  port: {
    valid: (v) => isRecord(v) && typeof v.number === 'number' && typeof v.proto === 'string',
    format: (v) => (isRecord(v) ? `${v.number}/${v.proto}` : fallbackValue(v)),
    example: '{ "t": "port", "v": { "number": 443, "proto": "tcp" } }',
  },
  line_range: {
    valid: (v) => isRecord(v) && typeof v.start === 'number' && typeof v.end === 'number',
    format: (v) => (isRecord(v) ? `${v.start}-${v.end}` : fallbackValue(v)),
    example: '{ "t": "line_range", "v": { "start": 1, "end": 20 } }',
  },
  http_route: {
    valid: (v) => isRecord(v) && typeof v.method === 'string' && typeof v.path === 'string',
    format: (v) => (isRecord(v) ? `${v.method} ${v.path}` : fallbackValue(v)),
    example: '{ "t": "http_route", "v": { "method": "GET", "path": "/" } }',
  },
  resource_id: {
    valid: (v) => isRecord(v) && typeof v.scheme === 'string' && typeof v.id === 'string',
    format: (v) => (isRecord(v) ? `${v.scheme}:${v.id}` : fallbackValue(v)),
    example: '{ "t": "resource_id", "v": { "scheme": "arn", "id": "…" } }',
  },
};

/** A readable rendering of any value for a coordinate whose shape is wrong. */
function fallbackValue(v: unknown): string {
  if (typeof v === 'string') return `"${v}"`;
  if (v === null || v === undefined) return 'null';
  if (typeof v === 'object') return JSON.stringify(v);
  return String(v);
}

// ---------------------------------------------------------------------------
// Parsed view model
// ---------------------------------------------------------------------------

export interface AssetCoordView {
  tag: string;
  /** Display string for the value. */
  text: string;
  /** False when `v`'s shape doesn't match what `tag` expects (an unknown tag
   *  is never flagged). */
  valid: boolean;
}

export interface AssetErrorView {
  /** The raw serde message, verbatim (prefix stripped). */
  raw: string;
  /** Plain-language explanation of what was expected. */
  friendly: string;
  /** A correct `{t, v}` example for the faulting tag, when known. */
  expected: string | null;
  /** The coordinate value the agent actually sent, when identifiable. */
  got: string | null;
}

export interface AssetView {
  kind: string | null;
  depth: string | null;
  label: string | null;
  coords: AssetCoordView[];
  /** Success: the ledger asset id and the effective (monotonic) depth. */
  assetId: string | null;
  effectiveDepth: string | null;
  error: AssetErrorView | null;
}

function coordViews(input: Record<string, unknown>): AssetCoordView[] {
  const raw = Array.isArray(input.coordinates) ? input.coordinates : [];
  const out: AssetCoordView[] = [];
  for (const c of raw) {
    if (!isRecord(c) || typeof c.t !== 'string') continue;
    const tag = c.t;
    const spec = COORD_SPECS[tag];
    const valid = spec ? spec.valid(c.v) : true;
    const text = spec ? spec.format(c.v) : fallbackValue(c.v);
    out.push({ tag, text, valid });
  }
  return out;
}

/** Variant name (`Coordinate::Port`) → its snake_case tag (`port`). */
function variantTag(variant: string): string {
  return variant.replace(/([a-z0-9])([A-Z])/g, '$1_$2').toLowerCase();
}

/** The tag a `missing field "x"` error points at (the only object-valued tags
 *  with required fields). */
function tagForMissingField(field: string): string | null {
  switch (field) {
    case 'number':
    case 'proto':
      return 'port';
    case 'start':
    case 'end':
      return 'line_range';
    case 'method':
      return 'http_route';
    case 'scheme':
      return 'resource_id';
    // `path` and `id` are ambiguous (also scalar tags) — leave unmapped.
    default:
      return null;
  }
}

function fieldsOf(tag: string): string {
  switch (tag) {
    case 'port':
      return 'number, proto';
    case 'line_range':
      return 'start, end';
    case 'http_route':
      return 'method, path';
    case 'resource_id':
      return 'scheme, id';
    default:
      return '…';
  }
}

/** Turn a raw `assets.mark` serde error into an "expected vs got" explanation.
 *  Recognises the two shapes agents actually hit — a wrong type for a
 *  struct-variant coordinate (`expected struct variant Coordinate::Port`) and
 *  a struct missing a field (`missing field \`number\``) — and falls back to
 *  the cleaned message otherwise. */
export function interpretAssetError(error: string, coords: AssetCoordView[]): AssetErrorView {
  const raw = error.replace(/^invalid input:\s*/i, '').trim();

  let badTag: string | null = null;
  const variant = raw.match(/Coordinate::(\w+)/);
  if (variant) badTag = variantTag(variant[1]);
  if (!badTag) {
    const missing = raw.match(/missing field `([^`]+)`/);
    if (missing) badTag = tagForMissingField(missing[1]);
  }

  if (badTag && COORD_SPECS[badTag]) {
    const spec = COORD_SPECS[badTag];
    // The offending coordinate's sent value — the first invalid one of that tag.
    const bad = coords.find((c) => c.tag === badTag && !c.valid) ?? coords.find((c) => c.tag === badTag);
    const friendly = `A ${badTag} coordinate must be an object \`{ ${fieldsOf(badTag)} }\`.`;
    return { raw, friendly, expected: spec.example ?? null, got: bad ? bad.text : null };
  }

  return { raw, friendly: raw, expected: null, got: null };
}

/** The success output: `asset <id> is at depth \`<depth>\``. */
function parseSuccessOutput(output: string): { assetId: string | null; effectiveDepth: string | null } {
  const m = output.match(/^asset (\S+) is at depth `([^`]+)`/);
  if (m) return { assetId: m[1], effectiveDepth: m[2] };
  return { assetId: null, effectiveDepth: null };
}

export function parseAsset(tool: ToolView): AssetView {
  const input = isRecord(tool.input) ? tool.input : {};
  const coords = coordViews(input);
  const view: AssetView = {
    kind: typeof input.kind === 'string' ? input.kind : null,
    depth: typeof input.depth === 'string' ? input.depth : null,
    label: typeof input.label === 'string' ? input.label : null,
    coords,
    assetId: null,
    effectiveDepth: null,
    error: null,
  };
  if (tool.error) {
    view.error = interpretAssetError(tool.error, coords);
  } else if (tool.output) {
    const { assetId, effectiveDepth } = parseSuccessOutput(tool.output);
    view.assetId = assetId;
    view.effectiveDepth = effectiveDepth;
  }
  return view;
}

// ---------------------------------------------------------------------------
// Components
// ---------------------------------------------------------------------------

/** Collapsed-row summary: kind + coordinate chips (the faulting one flagged) +
 *  depth / outcome. Always visible, so the row reads on its own. */
export function AssetSummary({ view }: { view: AssetView }) {
  return (
    <span className="flex min-w-0 flex-1 flex-wrap items-center gap-1">
      {view.kind && (
        <Chip tone="brand" mono>
          {view.kind}
        </Chip>
      )}
      {view.coords.map((c, i) => (
        <Chip
          key={i}
          tone={c.valid ? 'neutral' : 'err'}
          mono
          title={c.tag}
          className={c.valid ? undefined : 'ring-1 ring-inset ring-err/40'}
        >
          {c.tag === 'port' || c.tag === 'host' ? c.text : `${c.tag}: ${c.text}`}
        </Chip>
      ))}
      {view.effectiveDepth ? (
        <Chip tone="ok">depth {view.effectiveDepth}</Chip>
      ) : view.error ? (
        <Chip tone="err">rejected</Chip>
      ) : view.depth ? (
        <Chip tone="neutral">depth {view.depth}</Chip>
      ) : null}
    </span>
  );
}

/** Expanded body: the label, then the ledger line (success) or the
 *  expected-vs-got explanation (rejected). */
export function AssetBody({ view }: { view: AssetView }) {
  return (
    <div className="px-3 py-2 space-y-2">
      {view.label && <p className="text-ui text-ink leading-snug">{view.label}</p>}

      {view.error ? (
        <div className="space-y-1.5">
          <div className="rounded bg-surface px-2.5 py-2 text-note text-ink-dim leading-relaxed">
            <p>{view.error.friendly}</p>
            {view.error.got !== null && (
              <p className="mt-1">
                The agent sent{' '}
                <code className="rounded bg-err-bg px-1 py-px font-mono text-err">{view.error.got}</code>.
              </p>
            )}
            {view.error.expected && (
              <p className="mt-1">
                Expected e.g.{' '}
                <code className="rounded bg-brand-500/10 px-1 py-px font-mono text-brand-700">
                  {view.error.expected}
                </code>
              </p>
            )}
          </div>
          <p className="font-mono text-meta text-ink-mute">{view.error.raw}</p>
        </div>
      ) : (
        (view.assetId || view.effectiveDepth) && (
          <p className="font-mono text-meta text-ink-mute">
            {view.assetId && <>asset {view.assetId}</>}
            {view.assetId && view.effectiveDepth && ' · '}
            {view.effectiveDepth && (
              <>
                depth {view.effectiveDepth}
                {view.depth && view.depth !== view.effectiveDepth && (
                  <span className="text-ink-dim"> (requested {view.depth})</span>
                )}
              </>
            )}
          </p>
        )
      )}
    </div>
  );
}
