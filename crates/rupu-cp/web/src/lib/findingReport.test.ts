// @vitest-environment jsdom
import { describe, it, expect, vi, afterEach } from 'vitest';
import fixture from '../../../../rupu-coverage/tests/fixtures/finding_report/valid_full.json';
import { isSentinel, isGapSentinel, sentinelLabel, codeHref, copyText, formatBytes, completeness, type FindingReport } from './findingReport';
import { api, findingArtifactUrl } from './api';

afterEach(() => vi.restoreAllMocks());

describe('findingReport helpers', () => {
  it('classifies sentinels', () => {
    expect(isSentinel('None')).toBe(true);
    expect(isSentinel({ diff: 'x' })).toBe(false);
    expect(isGapSentinel('Unknown')).toBe(true);
    expect(isGapSentinel('Not Provided — needs hardware')).toBe(true);
    expect(isGapSentinel('None Provided')).toBe(false);
    expect(sentinelLabel('Not Provided — needs hardware')).toBe('Not provided: needs hardware');
    expect(sentinelLabel('Unknown')).toBe('Unknown');
  });

  it('builds code links', () => {
    expect(codeHref('ws 1', 'src/a b.rs', 12)).toBe('/projects/ws%201/code?path=src%2Fa%20b.rs&line=12');
    expect(codeHref('ws', 'x.rs')).toBe('/projects/ws/code?path=x.rs');
  });

  it('formats bytes', () => {
    expect(formatBytes(0)).toBe('0 B');
    expect(formatBytes(1536)).toBe('1.5 KB');
    expect(formatBytes(5 * 1024 * 1024)).toBe('5.0 MB');
  });

  it('copyText reports failure instead of throwing', async () => {
    Object.assign(navigator, { clipboard: { writeText: vi.fn().mockRejectedValue(new Error('denied')) } });
    await expect(copyText('x')).resolves.toBe(false);
    Object.assign(navigator, { clipboard: { writeText: vi.fn().mockResolvedValue(undefined) } });
    await expect(copyText('x')).resolves.toBe(true);
  });

  it('getFinding and artifact URLs encode ids', async () => {
    const fetchMock = vi.spyOn(globalThis, 'fetch').mockResolvedValue(new Response(JSON.stringify({ id: 'fnd_1' })));
    await api.getFinding('fnd_1');
    expect(fetchMock.mock.calls[0][0]).toBe('/api/findings/fnd_1');
    expect(findingArtifactUrl('fnd/1', 'ab')).toBe('/api/findings/fnd%2F1/artifacts/ab');
  });
});

describe('completeness', () => {
  const report = fixture as unknown as FindingReport;

  it('fixture gaps are owner and cvss_v3', () => {
    expect(completeness(report)).toEqual({ filled: 9, total: 11, gaps: ['owner', 'cvss_v3'] });
  });

  it('Not Provided sections are gaps but None-style answers are not', () => {
    const r: FindingReport = {
      ...report,
      regression_test: 'Not Provided — needs hardware',
      ownership: { ...report.ownership, source_repository: 'Not Applicable' },
      tickets: 'None Provided',
    };
    const { gaps } = completeness(r);
    expect(gaps).toContain('regression_test');
    expect(gaps).not.toContain('source_repository');
    expect(gaps).not.toContain('tickets');
  });

  it('an Unknown tickets sentinel is a gap, a ticket list is not', () => {
    expect(completeness({ ...report, tickets: 'Unknown' }).gaps).toContain('tickets');
    const withTicket = { ...report, tickets: [{ type: 'Jira', identifier: 'SEC-1' }] };
    expect(completeness(withTicket).gaps).not.toContain('tickets');
  });

  it('trims Unknown but requires the exact Not Provided prefix', () => {
    const r: FindingReport = { ...report, ownership: { ...report.ownership, product: '  Unknown ' }, recommended_patch: 'Not Provided — ' };
    const { gaps } = completeness(r);
    expect(gaps).toContain('product');
    expect(gaps).toContain('recommended_patch');
    expect(completeness({ ...report, ci_cd_detection: 'Not Provided' }).gaps).not.toContain('ci_cd_detection');
  });
});
