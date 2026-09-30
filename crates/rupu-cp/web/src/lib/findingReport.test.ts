// @vitest-environment jsdom
import { describe, it, expect, vi, afterEach } from 'vitest';
import { isSentinel, isGapSentinel, sentinelLabel, codeHref, copyText, formatBytes } from './findingReport';
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
