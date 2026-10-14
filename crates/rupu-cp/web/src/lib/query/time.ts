// Time values of a `time` query field — the TypeScript twin of rupu-query's
// `time.rs`, held in lockstep by `crates/rupu-query/tests/fixtures/
// time_values.json` (time.lockstep.test.ts). A value is a span back from now
// (`30m`, `12h`, `7d`, `2w`), a UTC date (`2026-10-01`, its whole day) or an
// RFC 3339 time. A span resolves against `now` when it is matched.

export type TimeValue =
  | { kind: 'ago'; ms: number }
  | { kind: 'day'; start: number }
  | { kind: 'at'; ms: number };

const DAY_MS = 86_400_000;
const UNIT_SECS: Record<string, bigint> = { m: 60n, h: 3_600n, d: 86_400n, w: 604_800n };
/** rupu-query's `MAX_SPAN_DAYS` (10000 years), in seconds. */
const MAX_SPAN_SECS = 3_652_425n * 86_400n;
/** chrono's `NaiveDate` year range. */
const MAX_YEAR = 262_143;

const isLeap = (y: number) => (y % 4 === 0 && y % 100 !== 0) || y % 400 === 0;
const daysIn = (y: number, m: number) => [31, isLeap(y) ? 29 : 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31][m - 1];

/** Days from 1970-01-01 to a proleptic Gregorian date (year 0 exists). */
function daysFromCivil(y: number, m: number, d: number): number {
  const yy = m <= 2 ? y - 1 : y;
  const era = Math.floor(yy / 400);
  const yoe = yy - era * 400;
  const doy = Math.floor((153 * (m + (m > 2 ? -3 : 9)) + 2) / 5) + d - 1;
  const doe = yoe * 365 + Math.floor(yoe / 4) - Math.floor(yoe / 100) + doy;
  return era * 146_097 + doe - 719_468;
}

function validDate(y: number, m: number, d: number): boolean {
  return Math.abs(y) <= MAX_YEAR && m >= 1 && m <= 12 && d >= 1 && d <= daysIn(y, m);
}

function parseSpan(v: string): number | null {
  const m = /^(\d+)([mhdw])$/i.exec(v);
  if (!m) return null;
  const secs = BigInt(m[1]) * UNIT_SECS[m[2].toLowerCase()];
  return secs > MAX_SPAN_SECS ? null : Number(secs) * 1000;
}

/** chrono's `%Y-%m-%d`: a year of one to four digits, or signed and any
 *  length; a month and a day of one or two digits. */
function parseDay(v: string): number | null {
  const m = /^([+-]\d+|\d{1,4})-(\d{1,2})-(\d{1,2})$/.exec(v);
  if (!m) return null;
  const [y, mo, d] = [Number(m[1]), Number(m[2]), Number(m[3])];
  return validDate(y, mo, d) ? daysFromCivil(y, mo, d) * DAY_MS : null;
}

/** An RFC 3339 time as epoch ms (a leap second reads as the end of its
 *  minute's 59th second), or null. */
export function parseRfc3339(v: string): number | null {
  const m = /^(\d{4})-(\d{2})-(\d{2})[Tt ](\d{2}):(\d{2}):(\d{2})(?:\.(\d+))?(?:([Zz])|([+-])(\d{2}):(\d{2}))$/.exec(v);
  if (!m) return null;
  const [y, mo, d, h, mi, s] = m.slice(1, 7).map(Number);
  if (!validDate(y, mo, d) || h > 23 || mi > 59 || s > 60) return null;
  let offset = 0;
  if (!m[8]) {
    const [oh, om] = [Number(m[10]), Number(m[11])];
    if (oh > 23 || om > 59) return null;
    offset = (m[9] === '-' ? -1 : 1) * (oh * 60 + om) * 60_000;
  }
  const frac = m[7] ? Number(`0.${m[7]}`) * 1000 : 0;
  const secMs = s === 60 ? 59_999 : s * 1000 + Math.floor(frac);
  return daysFromCivil(y, mo, d) * DAY_MS + h * 3_600_000 + mi * 60_000 + secMs - offset;
}

export function parseTimeValue(v: string): TimeValue | null {
  const span = parseSpan(v);
  if (span !== null) return { kind: 'ago', ms: span };
  const day = parseDay(v);
  if (day !== null) return { kind: 'day', start: day };
  const at = parseRfc3339(v);
  return at === null ? null : { kind: 'at', ms: at };
}

/** `since:` keeps a time at or after this value: a day from its start. */
export function admitsSince(t: TimeValue, ts: number, now: number): boolean {
  switch (t.kind) {
    case 'ago':
      return ts >= now - t.ms;
    case 'day':
      return ts >= t.start;
    case 'at':
      return ts >= t.ms;
  }
}

/** `until:` keeps a time at or before this value: a day through its end. */
export function admitsUntil(t: TimeValue, ts: number, now: number): boolean {
  switch (t.kind) {
    case 'ago':
      return ts <= now - t.ms;
    case 'day':
      return ts < t.start + DAY_MS;
    case 'at':
      return ts <= t.ms;
  }
}

/** Validate a time value at parse time: a span lowercased, anything else as written. */
export function normalizeTime(v: string): { ok: true; value: string } | { ok: false; message: string } {
  const t = parseTimeValue(v);
  if (t === null) {
    return { ok: false, message: `\`${v}\` is not a time (expected 30m, 12h, 7d, 2w, 2026-10-01 or an RFC 3339 time)` };
  }
  return { ok: true, value: t.kind === 'ago' ? v.toLowerCase() : v };
}
