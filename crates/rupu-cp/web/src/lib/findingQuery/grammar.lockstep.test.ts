// The TypeScript half of the finding-query lockstep: the same fixtures the
// Rust `finding_query_lockstep` test runs. If this fails, the two parsers
// drifted — fix the parser, never the fixture, unless the spec changed.
import { describe, expect, it } from 'vitest';
import { parseQuery } from './grammar';
import { FINDING_FIELDS } from './fields';
import casesJson from '../../../../../rupu-coverage/tests/fixtures/finding_query/cases.json';
import fieldsJson from '../../../../../rupu-coverage/tests/fixtures/finding_query/fields.json';

type Case = { q: string; terms?: unknown[]; error?: { token: number; code: string } };
const cases = casesJson as unknown as Case[];
const fields = fieldsJson as unknown as unknown[];

describe('finding query lockstep', () => {
  it.each(cases.map((c) => [c.q, c] as const))('%j', (_q, c) => {
    const r = parseQuery(c.q, FINDING_FIELDS);
    if (c.terms) {
      expect(r).toEqual({ ok: true, terms: c.terms });
    } else {
      expect(r.ok).toBe(false);
      if (!r.ok) expect({ token: r.error.token, code: r.error.code }).toEqual(c.error);
    }
  });

  it('registry matches fields.json', () => {
    expect(FINDING_FIELDS.map(({ key, aliases, kind, values }) => ({ key, aliases, kind, values }))).toEqual(fields);
  });
});
