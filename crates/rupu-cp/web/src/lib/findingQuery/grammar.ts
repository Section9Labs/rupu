// The findings query language — TypeScript twin of
// `crates/rupu-coverage/src/ledger/query_lang.rs`, generic over a field
// registry. The web only PARSES (chips, suggestions, inline errors); the
// server evaluates. Held in lockstep by the shared fixtures in
// `rupu-coverage/tests/fixtures/finding_query/` (grammar.lockstep.test.ts).
// Change both parsers and the fixtures together.

export type FieldKind = 'severity' | 'tag' | 'cwe' | 'enum' | 'text';
export interface FieldSpec {
  key: string;
  aliases: string[];
  kind: FieldKind;
  values: string[];
}
export type Op = 'eq' | 'gt' | 'ge' | 'lt' | 'le';
export interface Term {
  neg: boolean;
  key: string;
  op: Op;
  values: string[];
}
export type ErrorCode = 'unknown_key' | 'empty_value' | 'bad_value' | 'bad_operator' | 'unclosed_quote' | 'bad_quote';
export interface QueryError {
  token: number;
  start: number;
  end: number;
  code: ErrorCode;
  message: string;
}
export type ParseResult = { ok: true; terms: Term[] } | { ok: false; error: QueryError };
export interface RawToken {
  start: number;
  end: number;
  text: string;
}

// Rust's `char::is_whitespace` (Unicode White_Space).
const WS = /^[\t\n\v\f\r \u0085\u00a0\u1680\u2000-\u200a\u2028\u2029\u202f\u205f\u3000]$/u;
const isWs = (c: string) => WS.test(c);
const isKeyChar = (c: string | undefined) => c !== undefined && /^[A-Za-z_]$/.test(c);

function trimWs(s: string): string {
  const cs = Array.from(s);
  let a = 0;
  let b = cs.length;
  while (a < b && isWs(cs[a])) a++;
  while (b > a && isWs(cs[b - 1])) b--;
  return cs.slice(a, b).join('');
}

type Scanned = { start: number; end: number; chars: string[] };
type ScanOut = { ok: true; tokens: Scanned[] } | { ok: false; error: QueryError };

/**
 * `keyed_value_start`: where a keyed token's value begins — the index right
 * after the operator when the token (after an optional leading `-`) is
 * `[A-Za-z_]+` followed immediately by an operator (`>=` `<=` `:` `>` `<`,
 * longest match first). Null when the token is not keyed.
 */
function keyedValueStart(chars: string[], start: number): number | null {
  const body = start + (chars[start] === '-' ? 1 : 0);
  let keyLen = 0;
  while (isKeyChar(chars[body + keyLen])) keyLen++;
  if (keyLen === 0) return null;
  const k = body + keyLen;
  const a = chars[k];
  const b = chars[k + 1];
  if ((a === '>' || a === '<') && b === '=') return k + 2;
  if (a === '>' || a === '<' || a === ':') return k + 1;
  return null;
}

/**
 * `scan`: split into tokens at unquoted whitespace. A quote opens only at an
 * item start: the token body start (after an optional `-`), right after a
 * keyed token's operator, or right after an unescaped, unquoted `,` inside a
 * keyed value. `\` escapes the next char, in or out of quotes.
 */
function scan(q: string): ScanOut {
  const chars = Array.from(q);
  const tokens: Scanned[] = [];
  let i = 0;
  while (i < chars.length) {
    if (isWs(chars[i])) {
      i++;
      continue;
    }
    const start = i;
    const valueStart = keyedValueStart(chars, start);
    let itemStart = true;
    let quote: string | null = null;
    while (i < chars.length) {
      const c = chars[i];
      if (quote !== null) {
        if (c === '\\') {
          i += 2;
          continue;
        }
        if (c === quote) {
          quote = null;
          itemStart = false;
        }
        i++;
        continue;
      }
      if (isWs(c)) break;
      if (c === '\\') {
        i += 2;
        itemStart = false;
        continue;
      }
      if (itemStart && (c === '"' || c === "'")) {
        quote = c;
        i++;
        continue;
      }
      itemStart =
        (c === '-' && i === start) ||
        (valueStart !== null && (i + 1 === valueStart || (c === ',' && i >= valueStart)));
      i++;
    }
    const end = Math.min(i, chars.length);
    if (quote !== null) {
      return {
        ok: false,
        error: { token: tokens.length, start, end, code: 'unclosed_quote', message: 'this quote is never closed' },
      };
    }
    tokens.push({ start, end, chars: chars.slice(start, end) });
    i = end;
  }
  return { ok: true, tokens };
}

export function tokenize(q: string): { ok: true; tokens: RawToken[] } | { ok: false; error: QueryError } {
  const s = scan(q);
  if (!s.ok) return s;
  return { ok: true, tokens: s.tokens.map((t) => ({ start: t.start, end: t.end, text: t.chars.join('') })) };
}

type ItemsOut = { ok: true; values: string[] } | { ok: false; code: ErrorCode; message: string };

/** `items`: decode items (escapes resolved, quotes removed); with `split`, an unescaped, unquoted `,` separates them. */
function items(s: string[], split: boolean): ItemsOut {
  const out: string[] = [];
  let i = 0;
  for (;;) {
    let cur = '';
    if (i < s.length && (s[i] === '"' || s[i] === "'")) {
      const qc = s[i];
      i++;
      for (;;) {
        if (i >= s.length) return { ok: false, code: 'unclosed_quote', message: 'this quote is never closed' };
        const c = s[i];
        if (c === '\\' && i + 1 < s.length) {
          cur += s[i + 1];
          i += 2;
          continue;
        }
        i++;
        if (c === qc) break;
        cur += c;
      }
      if (i < s.length && !(split && s[i] === ',')) {
        return { ok: false, code: 'bad_quote', message: 'put a space or a comma after a closing quote' };
      }
    } else {
      while (i < s.length) {
        const c = s[i];
        if (c === '\\' && i + 1 < s.length) {
          cur += s[i + 1];
          i += 2;
          continue;
        }
        if (split && c === ',') break;
        cur += c;
        i++;
      }
    }
    if (cur === '') return { ok: false, code: 'empty_value', message: 'empty value' };
    out.push(cur);
    if (split && i < s.length && s[i] === ',') {
      i++;
      if (i === s.length) return { ok: false, code: 'empty_value', message: 'empty value after a comma' };
      continue;
    }
    return { ok: true, values: out };
  }
}

const asciiLower = (s: string) => s.replace(/[A-Z]/g, (c) => c.toLowerCase());

/** `Tag::parse` (rupu-coverage `ledger/tags.rs`). */
export function parseTag(raw: string): { ok: true; tag: string } | { ok: false; message: string } {
  const bad = (reason: string) => ({ ok: false as const, message: `invalid tag \`${raw}\`: ${reason}` });
  const t = trimWs(raw).toLowerCase();
  if (t === '') return bad('empty');
  if (!/^[a-z0-9._:/-]*$/.test(t)) return bad('only a-z, 0-9 and . _ : / - are allowed');
  if (!/^[a-z0-9]/.test(t)) return bad('must start with a letter or digit');
  if (t.length > 64) return bad('longer than 64 characters');
  return { ok: true, tag: t };
}

/** `parse_cwe` (rupu-coverage `report/cwe.rs`). */
export function parseCwe(raw: string): number | null {
  const s = trimWs(raw);
  let digits = s;
  if (asciiLower(s.slice(0, 3)) === 'cwe') {
    const rest = s.slice(3);
    digits = rest.startsWith('-') || rest.startsWith('_') ? rest.slice(1) : rest;
  }
  if (!/^[0-9]+$/.test(digits)) return null;
  const n = Number(digits);
  return Number.isSafeInteger(n) && n <= 4294967295 ? n : null;
}

/** A value in the canonical form the query compares (tags lowercased, `79`
 *  as `CWE-79`, enum values lowercased), or an error for a value the key
 *  doesn't take. */
export function normalize(f: FieldSpec, v: string): { ok: true; value: string } | { ok: false; message: string } {
  switch (f.kind) {
    case 'severity':
    case 'enum': {
      const l = asciiLower(v);
      return f.values.includes(l)
        ? { ok: true, value: l }
        : { ok: false, message: `\`${v}\` is not a ${f.key} (one of ${f.values.join(', ')})` };
    }
    case 'tag': {
      const t = parseTag(v);
      return t.ok ? { ok: true, value: t.tag } : { ok: false, message: t.message };
    }
    case 'cwe': {
      const n = parseCwe(v);
      return n === null
        ? { ok: false, message: `\`${v}\` is not a CWE id (expected 79 or CWE-79)` }
        : { ok: true, value: `CWE-${n}` };
    }
    default:
      return { ok: true, value: v };
  }
}

export function findField(name: string, fields: readonly FieldSpec[]): FieldSpec | undefined {
  const n = asciiLower(name);
  return fields.find((f) => f.key === n || f.aliases.includes(n));
}

function parseRaw(t: Scanned, idx: number, fields: readonly FieldSpec[]): ParseResult {
  const fail = (code: ErrorCode, message: string): ParseResult => ({
    ok: false,
    error: { token: idx, start: t.start, end: t.end, code, message },
  });
  const neg = t.chars.length > 1 && t.chars[0] === '-';
  const body = neg ? t.chars.slice(1) : t.chars;
  let keyLen = 0;
  while (keyLen < body.length && isKeyChar(body[keyLen])) keyLen++;
  const rest = body.slice(keyLen);
  let op: Op | null = null;
  let opLen = 0;
  if (keyLen > 0) {
    if (rest[0] === '>' && rest[1] === '=') [op, opLen] = ['ge', 2];
    else if (rest[0] === '<' && rest[1] === '=') [op, opLen] = ['le', 2];
    else if (rest[0] === '>') [op, opLen] = ['gt', 1];
    else if (rest[0] === '<') [op, opLen] = ['lt', 1];
    else if (rest[0] === ':') [op, opLen] = ['eq', 1];
  }
  if (op === null) {
    const it = items(body, false);
    if (!it.ok) return fail(it.code, it.message);
    return { ok: true, terms: [{ neg, key: 'text', op: 'eq', values: it.values }] };
  }
  const name = body.slice(0, keyLen).join('');
  const f = findField(name, fields);
  if (!f) return fail('unknown_key', `unknown key \`${name}\` (quote the text to search for it)`);
  if (op !== 'eq' && f.kind !== 'severity') return fail('bad_operator', `\`${f.key}\` only takes \`:\``);
  const it = items(rest.slice(opLen), true);
  if (!it.ok) return fail(it.code, it.message);
  if (op !== 'eq' && it.values.length > 1) return fail('bad_operator', 'a comparison takes one value');
  const values: string[] = [];
  for (const v of it.values) {
    const n = normalize(f, v);
    if (!n.ok) return fail('bad_value', n.message);
    values.push(n.value);
  }
  return { ok: true, terms: [{ neg, key: f.key, op, values }] };
}

export function parseQuery(q: string, fields: readonly FieldSpec[]): ParseResult {
  const s = scan(q);
  if (!s.ok) return s;
  const terms: Term[] = [];
  for (let i = 0; i < s.tokens.length; i++) {
    const r = parseRaw(s.tokens[i], i, fields);
    if (!r.ok) return r;
    terms.push(...r.terms);
  }
  return { ok: true, terms };
}

/** Parse one chip's raw text as a one-token query. */
export function parseToken(raw: string, fields: readonly FieldSpec[]): ParseResult {
  return parseQuery(raw, fields);
}

/** A value as the grammar must spell it: bare when it can be, else quoted.
 *  It must be quoted when it holds a separator the scanner splits on
 *  (`isWs`, not JS `\s`, which lacks U+0085) or a `,`, quote or `\`. */
export function quoteValue(v: string): string {
  if (v !== '' && !Array.from(v).some((c) => isWs(c) || `,"'\\`.includes(c))) return v;
  return `"${v.replace(/["\\]/g, (c) => `\\${c}`)}"`;
}
