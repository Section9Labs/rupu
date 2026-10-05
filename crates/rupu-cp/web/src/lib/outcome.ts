/**
 * TS port of `rupu_transcript::outcome::recovery_line` — the one-line phrase
 * for a recovery action, kept identical to the CLI's. Plan 3 replaces both
 * with the full presentation module.
 */

export interface RecoveryLineInput {
  rung: number;
  action: string;
  attempt?: number | null;
  budget?: number | null;
  provider?: string | null;
  model?: string | null;
  reason?: string | null;
}

function target(provider?: string | null, model?: string | null): string {
  if (provider && model) return `${provider}/${model}`;
  return model || provider || '?';
}

export function recoveryLine(r: RecoveryLineInput): string {
  let phrase: string;
  switch (r.action) {
    case 'continued':
      phrase =
        r.attempt != null && r.budget != null ? `continued ${r.attempt}/${r.budget}` : 'continued';
      break;
    case 'fell_back':
      phrase = `fell back to ${target(r.provider, r.model)}`;
      break;
    case 'served_by_fallback':
      phrase = `served by ${r.model || target(r.provider, r.model)}`;
      break;
    case 'skipped':
      phrase = r.reason
        ? `skipped ${target(r.provider, r.model)}: ${r.reason}`
        : `skipped ${target(r.provider, r.model)}`;
      break;
    case 'failed':
      phrase = 'no recovery left';
      break;
    default:
      // retried, compacted, asked, parked — and any action a newer writer adds.
      phrase = r.action;
  }
  return `↺ rung ${r.rung} · ${phrase}`;
}

/** Cap on the raw JSON shown on an unrecognized block's row. */
const BLOCK_JSON_MAX_CHARS = 200;

/** Compact JSON with object keys sorted, matching serde_json's output. */
function sortedJson(v: unknown): string {
  if (Array.isArray(v)) return `[${v.map(sortedJson).join(',')}]`;
  if (v !== null && typeof v === 'object') {
    const rec = v as Record<string, unknown>;
    return `{${Object.keys(rec)
      .sort()
      .map((k) => `${JSON.stringify(k)}:${sortedJson(rec[k])}`)
      .join(',')}}`;
  }
  return JSON.stringify(v) ?? 'null';
}

function cutJson(v: unknown): string {
  const json = sortedJson(v);
  const chars = Array.from(json);
  return chars.length <= BLOCK_JSON_MAX_CHARS ? json : `${chars.slice(0, BLOCK_JSON_MAX_CHARS - 1).join('')}…`;
}

/**
 * TS port of `rupu_transcript::outcome::assistant_block_line` — the one-line
 * text for an `assistant_block` event (a reply block with no event of its
 * own): a server-side fallback boundary, an unrecognized block, or a block
 * abandoned at a mid-output fallback. Plan 3 restyles these rows.
 */
export function assistantBlockLine(block: unknown, abandoned: boolean): string {
  const rec = (block !== null && typeof block === 'object' ? block : {}) as Record<string, unknown>;
  const str = (r: Record<string, unknown>, k: string) => (typeof r[k] === 'string' ? (r[k] as string) : null);
  const kind = str(rec, 'type') ?? '?';
  let body: string;
  switch (kind) {
    case 'fallback':
      body = `served by fallback · ${str(rec, 'from_model') ?? '?'} → ${str(rec, 'to_model') ?? '?'}`;
      break;
    case 'unknown': {
      const raw = rec.raw;
      const rawRec = (raw !== null && typeof raw === 'object' ? raw : {}) as Record<string, unknown>;
      const head = `unrecognized block · ${str(rawRec, 'type') ?? '?'}`;
      body = raw === undefined || raw === null ? head : `${head} ${cutJson(raw)}`;
      break;
    }
    case 'tool_use': {
      const name = str(rec, 'name');
      body = name ? `tool call · ${name}` : 'tool call';
      break;
    }
    default:
      body = kind;
  }
  return abandoned ? `abandoned · ${body}` : body;
}
