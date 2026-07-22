// Error messages on the event stream (`step_failed` / `run_failed`) are free
// strings that are OFTEN a serialized JSON object/array (provider errors, tool
// results, structured failures). Rendered raw they read as an unparsed blob.
// parseErrorDetail detects JSON — whole-string first, then a JSON substring
// embedded in a prefixed message — so the UI can offer a pretty "Parsed" view
// alongside the untouched "Raw" text.

export interface ParsedErrorDetail {
  /** The original string, always preserved for the Raw view. */
  raw: string;
  /** Present when a JSON value was recovered (whole or embedded). */
  json?: unknown;
  /** Pretty-printed `json` (2-space), present iff `json` is. */
  pretty?: string;
  /** Any non-JSON text preceding the embedded JSON (e.g. "provider error: {…}"). */
  prefix?: string;
}

function tryParse(s: string): unknown | undefined {
  try {
    return JSON.parse(s);
  } catch {
    return undefined;
  }
}

/** Recover a JSON object/array from `raw`, whole or embedded. Scalars (a bare
 *  number/`true`/quoted string) are NOT treated as "JSON worth pretty-printing"
 *  — only objects and arrays. */
export function parseErrorDetail(raw: string): ParsedErrorDetail {
  const trimmed = raw.trim();

  const whole = tryParse(trimmed);
  if (whole !== null && (typeof whole === 'object')) {
    return { raw, json: whole, pretty: JSON.stringify(whole, null, 2) };
  }

  // Embedded: find the first opening bracket and its matching last closing one.
  const firstObj = trimmed.indexOf('{');
  const firstArr = trimmed.indexOf('[');
  const candidates: Array<[number, string]> = [];
  if (firstObj >= 0) candidates.push([firstObj, '}']);
  if (firstArr >= 0) candidates.push([firstArr, ']']);
  // Prefer whichever bracket appears first.
  candidates.sort((a, b) => a[0] - b[0]);
  for (const [start, close] of candidates) {
    const end = trimmed.lastIndexOf(close);
    if (end <= start) continue;
    const slice = trimmed.slice(start, end + 1);
    const parsed = tryParse(slice);
    if (parsed !== null && typeof parsed === 'object') {
      const prefix = trimmed.slice(0, start).trim();
      return { raw, json: parsed, pretty: JSON.stringify(parsed, null, 2), prefix: prefix || undefined };
    }
  }

  return { raw };
}
