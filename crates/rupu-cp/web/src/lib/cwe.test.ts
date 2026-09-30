import { describe, it, expect } from 'vitest';
import { cweFromFinding, cweRef, findingCweIds } from './cwe';

describe('cweFromFinding', () => {
  it('derives CWE from a concern_id', () => {
    expect(
      cweFromFinding({ concern_id: 'cwe-top25-2023:cwe-787-out-of-bounds-write' }),
    ).toEqual({ id: 'CWE-787', url: 'https://cwe.mitre.org/data/definitions/787.html' });
  });

  it('falls back to an evidence reference URL', () => {
    expect(
      cweFromFinding({
        evidence: { references: ['https://cwe.mitre.org/data/definitions/798.html'] },
      }),
    ).toEqual({ id: 'CWE-798', url: 'https://cwe.mitre.org/data/definitions/798.html' });
  });

  it('returns null when neither source has a CWE', () => {
    expect(cweFromFinding({})).toBeNull();
  });

  it('returns null for a non-CWE concern_id with no CWE ref', () => {
    expect(
      cweFromFinding({
        concern_id: 'owasp-top10-2021:a01-broken-access-control',
        evidence: { references: ['https://owasp.org/Top10/A01'] },
      }),
    ).toBeNull();
  });
});

describe('cweRef', () => {
  it('accepts CWE-n, bare n and cwe_n spellings', () => {
    const want = { id: 'CWE-639', url: 'https://cwe.mitre.org/data/definitions/639.html' };
    expect(cweRef('CWE-639')).toEqual(want);
    expect(cweRef('639')).toEqual(want);
    expect(cweRef('cwe_639')).toEqual(want);
  });

  it('returns null with no number', () => {
    expect(cweRef('unknown')).toBeNull();
  });
});

describe('findingCweIds', () => {
  it('unions report_summary.cwe with the concern-derived id, deduplicated', () => {
    expect(
      findingCweIds({
        report_summary: { cwe: ['CWE-639', '862'] },
        concern_id: 'cwe-top25:cwe-639-idor',
      }),
    ).toEqual(['CWE-639', 'CWE-862']);
    expect(findingCweIds({ concern_id: 'cwe-top25:cwe-787-oob' })).toEqual(['CWE-787']);
    expect(findingCweIds({})).toEqual([]);
  });
});
