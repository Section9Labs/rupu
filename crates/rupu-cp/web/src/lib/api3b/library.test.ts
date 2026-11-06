import { afterEach, describe, expect, it, vi } from 'vitest';
import { ApiError } from '../api';
import {
  clearQueryFieldsCache,
  getFindingSchema,
  getQueryFields,
  importFindings,
  installSkills,
  previewSkillInstall,
  rerunCoverage,
  updateSkills,
  validateSkill,
} from './library';

afterEach(() => {
  vi.restoreAllMocks();
  clearQueryFieldsCache();
});

function jsonResponse(body: unknown, status = 200) {
  return new Response(JSON.stringify(body), { status });
}

function call(fetchMock: ReturnType<typeof vi.spyOn>, n = 0): { url: string; method?: string; body: unknown } {
  const [url, init] = fetchMock.mock.calls[n] as [string, RequestInit | undefined];
  return { url, method: init?.method, body: init?.body ? JSON.parse(init.body as string) : undefined };
}

describe('skills', () => {
  it('previews an install with dry_run and drops an empty ref', async () => {
    const preview = { dry_run: true, layer: 'global', found: [{ name: 'nmap', rel: 'nmap', description: 'd', status: 'new' }] };
    const fetchMock = vi.spyOn(globalThis, 'fetch').mockResolvedValue(jsonResponse(preview));

    const got = await previewSkillInstall({ source: ' example/skills ', ref: '  ' });

    expect(call(fetchMock)).toEqual({
      url: '/api/skills/install',
      method: 'POST',
      body: { source: 'example/skills', dry_run: true },
    });
    expect(got).toEqual(preview);
  });

  it('installs the chosen skills into a project with force', async () => {
    const fetchMock = vi.spyOn(globalThis, 'fetch').mockResolvedValue(jsonResponse({ installed: [], skipped: [] }));

    await installSkills({ source: 'example/skills', ref: 'v2', project: 'ws_1', skills: ['nmap'], force: true });

    expect(call(fetchMock).body).toEqual({ source: 'example/skills', ref: 'v2', project: 'ws_1', skills: ['nmap'], force: true });
  });

  it('carries a 409 as an ApiError with the server message', async () => {
    vi.spyOn(globalThis, 'fetch').mockImplementation(async () => jsonResponse({ error: 'nmap already exists' }, 409));

    await expect(installSkills({ source: 's', skills: ['nmap'], force: false })).rejects.toMatchObject({
      status: 409,
    });
    await expect(installSkills({ source: 's', skills: ['nmap'], force: false })).rejects.toBeInstanceOf(ApiError);
  });

  it('updates one skill and returns its result rows', async () => {
    const fetchMock = vi
      .spyOn(globalThis, 'fetch')
      .mockResolvedValue(jsonResponse({ layer: 'project', results: [{ name: 'nmap', outcome: 'up_to_date' }] }));

    const got = await updateSkills({ names: ['nmap'], project: 'ws_1' });

    expect(call(fetchMock).body).toEqual({ names: ['nmap'], project: 'ws_1' });
    expect(got).toEqual([{ name: 'nmap', outcome: 'up_to_date' }]);
  });

  it('validates an installed skill by layer and project', async () => {
    const fetchMock = vi
      .spyOn(globalThis, 'fetch')
      .mockResolvedValue(jsonResponse({ ok: true, skills: [{ name: 'nmap', path: '/p', status: 'ok', warnings: [] }] }));

    const got = await validateSkill({ name: 'nmap', layer: 'project', wsId: 'ws_1' });

    expect(call(fetchMock).body).toEqual({ name: 'nmap', layer: 'project', ws_id: 'ws_1' });
    expect(got.ok).toBe(true);
  });
});

describe('findings', () => {
  it('imports with dry_run, then for real', async () => {
    const report = { dry_run: true, files: [], backups: [], attached: 0, skipped: 0, failed: 0 };
    const fetchMock = vi.spyOn(globalThis, 'fetch').mockImplementation(async () => jsonResponse(report));

    await importFindings(['/srv/reports'], true);
    await importFindings(['/srv/reports'], false);

    expect(call(fetchMock, 0)).toEqual({ url: '/api/findings/import', method: 'POST', body: { paths: ['/srv/reports'], dry_run: true } });
    expect(call(fetchMock, 1).body).toEqual({ paths: ['/srv/reports'], dry_run: false });
  });

  it('reads the report schema', async () => {
    const fetchMock = vi.spyOn(globalThis, 'fetch').mockResolvedValue(jsonResponse({ title: 'FindingReport' }));

    expect(await getFindingSchema()).toEqual({ title: 'FindingReport' });
    expect(call(fetchMock).url).toBe('/api/findings/schema');
  });
});

describe('coverage', () => {
  it('reruns a recorded run in its workspace', async () => {
    const started = { run_id: 'run_2', host_id: 'local', agent: 'a', target_id: 't/x', ws_id: 'ws_1', replays: 'run_1' };
    const fetchMock = vi.spyOn(globalThis, 'fetch').mockResolvedValue(jsonResponse(started, 202));

    expect(await rerunCoverage('t/x', 'run_1', 'ws_1')).toEqual(started);
    expect(call(fetchMock)).toEqual({ url: '/api/coverage/t%2Fx/rerun?ws_id=ws_1', method: 'POST', body: { run: 'run_1' } });
  });
});

describe('query fields', () => {
  it('fetches a list registry once and serves it from the cache after', async () => {
    const fields = [{ key: 'status', aliases: [], kind: 'enum', values: ['failed'] }];
    const fetchMock = vi.spyOn(globalThis, 'fetch').mockImplementation(async () => jsonResponse({ list: 'sessions', fields }));

    expect(await getQueryFields('sessions')).toEqual(fields);
    expect(await getQueryFields('sessions')).toEqual(fields);

    expect(fetchMock).toHaveBeenCalledTimes(1);
    expect(call(fetchMock).url).toBe('/api/query/sessions/fields');
  });

  it('asks again after a failure', async () => {
    const fetchMock = vi
      .spyOn(globalThis, 'fetch')
      .mockResolvedValueOnce(jsonResponse({ error: 'down' }, 502))
      .mockResolvedValueOnce(jsonResponse({ list: 'events', fields: [] }));

    await expect(getQueryFields('events')).rejects.toBeInstanceOf(ApiError);
    expect(await getQueryFields('events')).toEqual([]);
    expect(fetchMock).toHaveBeenCalledTimes(2);
  });
});
