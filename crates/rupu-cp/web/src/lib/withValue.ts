// Smart-parse for a connector `with:` param field. Connector values may be
// literals (numbers, bools, lists) OR runtime minijinja templates ({{ ... }}),
// which the backend renders at run time and does NOT type-check at parse time.
// So: a JSON literal is stored as its typed value; anything else — a template
// or a plain word — is kept verbatim as a string.

/** Parse a `with:` field's text into its stored value. Empty/whitespace →
 *  `undefined` (the caller deletes the key). A valid JSON document parses to
 *  its typed value (number/bool/null/array/object, and a JSON-quoted string
 *  stays a string); anything else — a `{{ template }}` or a bare word — is
 *  returned verbatim (untrimmed) as a string. */
export function parseWithValue(text: string): unknown {
  if (text.trim() === '') return undefined;
  try {
    return JSON.parse(text);
  } catch {
    return text;
  }
}

/** Render a stored `with:` value back into its text field. Strings show
 *  verbatim; everything else shows its JSON text (so `3`/`true`/`["a"]`
 *  round-trip through `parseWithValue`). */
export function formatWithValue(v: unknown): string {
  return typeof v === 'string' ? v : JSON.stringify(v);
}
