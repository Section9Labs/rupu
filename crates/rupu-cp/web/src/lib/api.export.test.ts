/**
 * Finding-report export client: `findingExportUrl`, `api.exportFindings`
 * (a binary POST that cannot go through the JSON `request` wrapper) and the
 * `Content-Disposition` filename parser it uses.
 */

import { afterEach, describe, expect, it, vi } from 'vitest';
import {
  api,
  ApiError,
  apiErrorMessage,
  findingExportUrl,
  parseContentDispositionFilename,
} from './api';

afterEach(() => {
  vi.unstubAllGlobals();
});

function stubFetch(res: Response): ReturnType<typeof vi.fn> {
  const fn = vi.fn().mockResolvedValue(res);
  vi.stubGlobal('fetch', fn);
  return fn;
}

describe('findingExportUrl', () => {
  it('builds the per-finding export link for each format', () => {
    expect(findingExportUrl('fnd_1', 'md')).toBe('/api/findings/fnd_1/export?format=md');
    expect(findingExportUrl('fnd_1', 'html')).toBe('/api/findings/fnd_1/export?format=html');
    expect(findingExportUrl('fnd_1', 'pdf')).toBe('/api/findings/fnd_1/export?format=pdf');
  });

  it('percent-encodes the id', () => {
    expect(findingExportUrl('a/b c', 'md')).toBe('/api/findings/a%2Fb%20c/export?format=md');
  });
});

describe('parseContentDispositionFilename', () => {
  it('prefers the RFC 5987 filename* over filename', () => {
    expect(
      parseContentDispositionFilename(
        `attachment; filename="fallback.md"; filename*=UTF-8''r%C3%A9port%20one.md`,
      ),
    ).toBe('réport one.md');
  });

  it('reads a quoted filename', () => {
    expect(parseContentDispositionFilename('attachment; filename="my report.pdf"')).toBe('my report.pdf');
  });

  it('reads an unquoted token filename', () => {
    expect(parseContentDispositionFilename('attachment; filename=report.zip')).toBe('report.zip');
  });

  it('falls back to filename when filename* is malformed', () => {
    expect(
      parseContentDispositionFilename(`attachment; filename*=UTF-8''%E0%A4%A; filename="ok.md"`),
    ).toBe('ok.md');
  });

  it('keeps only the base name (no path components)', () => {
    expect(parseContentDispositionFilename('attachment; filename="../../etc/passwd"')).toBe('passwd');
    expect(parseContentDispositionFilename('attachment; filename="C:\\temp\\r.md"')).toBe('r.md');
  });

  it('returns null when there is no usable filename', () => {
    expect(parseContentDispositionFilename(null)).toBeNull();
    expect(parseContentDispositionFilename('')).toBeNull();
    expect(parseContentDispositionFilename('attachment')).toBeNull();
    expect(parseContentDispositionFilename('attachment; filename=""')).toBeNull();
  });
});

describe('api.exportFindings', () => {
  it('POSTs the body as JSON to /api/findings/export', async () => {
    const fetchMock = stubFetch(new Response('x', { status: 200 }));
    const body = { format: 'pdf' as const, title: 'T', ids: ['a', 'b'], include_summaries: true, split: true };
    await api.exportFindings(body);

    expect(fetchMock).toHaveBeenCalledTimes(1);
    const [url, init] = fetchMock.mock.calls[0] as [string, RequestInit];
    expect(url).toBe('/api/findings/export');
    expect(init.method).toBe('POST');
    expect(init.credentials).toBe('same-origin');
    expect(init.headers).toEqual({ 'Content-Type': 'application/json' });
    expect(JSON.parse(init.body as string)).toEqual(body);
  });

  it('returns the response bytes as a Blob', async () => {
    stubFetch(new Response('# report', { status: 200, headers: { 'Content-Type': 'text/markdown' } }));
    const blob = await api.exportFindings({ format: 'md' });
    expect(blob).toBeInstanceOf(Blob);
    expect(await blob.text()).toBe('# report');
  });

  it("names the result after the server's Content-Disposition filename", async () => {
    stubFetch(
      new Response('zipbytes', {
        status: 200,
        headers: { 'Content-Disposition': `attachment; filename="notebin-findings.zip"` },
      }),
    );
    const blob = await api.exportFindings({ format: 'md', split: true });
    expect(blob).toBeInstanceOf(File);
    expect((blob as File).name).toBe('notebin-findings.zip');
    expect(await blob.text()).toBe('zipbytes');
  });

  it('throws ApiError with the server error text on a non-2xx response', async () => {
    stubFetch(
      new Response(JSON.stringify({ error: 'no findings match this selection' }), {
        status: 404,
        statusText: 'Not Found',
      }),
    );
    const err = await api.exportFindings({ format: 'md', ids: ['zzz'] }).catch((e: unknown) => e);
    expect(err).toBeInstanceOf(ApiError);
    expect((err as ApiError).status).toBe(404);
    expect((err as ApiError).message).toContain('no findings match this selection');
    expect(apiErrorMessage(err)).toBe('no findings match this selection');
  });

  it('falls back to the status text when the error body is empty', async () => {
    stubFetch(new Response('', { status: 501, statusText: 'Not Implemented' }));
    const err = await api.exportFindings({ format: 'pdf' }).catch((e: unknown) => e);
    expect(err).toBeInstanceOf(ApiError);
    expect((err as ApiError).status).toBe(501);
    expect((err as ApiError).message).toBe('Not Implemented');
  });
});

describe('api.exportFindings abort', () => {
  it('hands the AbortSignal to fetch so a stalled export can be cancelled', async () => {
    const fetchMock = stubFetch(new Response('x', { status: 200 }));
    const controller = new AbortController();
    await api.exportFindings({ format: 'md' }, { signal: controller.signal });
    const [, init] = fetchMock.mock.calls[0] as [string, RequestInit];
    expect(init.signal).toBe(controller.signal);
  });
});

describe('api.downloadFindingExport', () => {
  it('GETs the per-finding export URL and returns the bytes', async () => {
    const fetchMock = stubFetch(new Response('# one finding', { status: 200 }));
    const blob = await api.downloadFindingExport('fnd_1', 'md');
    const [url, init] = fetchMock.mock.calls[0] as [string, RequestInit];
    expect(url).toBe('/api/findings/fnd_1/export?format=md');
    expect(init.credentials).toBe('same-origin');
    expect(init.method ?? 'GET').toBe('GET');
    expect(await blob.text()).toBe('# one finding');
  });

  it("names the result after the server's Content-Disposition filename", async () => {
    stubFetch(
      new Response('pdfbytes', {
        status: 200,
        headers: { 'Content-Disposition': `attachment; filename*=UTF-8''SEC-001.pdf` },
      }),
    );
    const blob = await api.downloadFindingExport('fnd_1', 'pdf');
    expect(blob).toBeInstanceOf(File);
    expect((blob as File).name).toBe('SEC-001.pdf');
  });

  it('throws ApiError with the server message on a non-2xx response', async () => {
    stubFetch(
      new Response(JSON.stringify({ error: 'this build was compiled without PDF support' }), {
        status: 501,
        statusText: 'Not Implemented',
      }),
    );
    const err = await api.downloadFindingExport('fnd_1', 'pdf').catch((e: unknown) => e);
    expect(err).toBeInstanceOf(ApiError);
    expect((err as ApiError).status).toBe(501);
    expect(apiErrorMessage(err)).toBe('this build was compiled without PDF support');
  });

  it('hands the AbortSignal to fetch', async () => {
    const fetchMock = stubFetch(new Response('x', { status: 200 }));
    const controller = new AbortController();
    await api.downloadFindingExport('fnd_1', 'html', { signal: controller.signal });
    const [, init] = fetchMock.mock.calls[0] as [string, RequestInit];
    expect(init.signal).toBe(controller.signal);
  });
});
