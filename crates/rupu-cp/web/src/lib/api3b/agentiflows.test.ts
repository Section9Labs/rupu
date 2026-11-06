import { describe, it, expect, vi, afterEach } from 'vitest';
import { agentiflowsApi, defSelectorFor, type AgentiflowDefRow } from './agentiflows';

afterEach(() => vi.restoreAllMocks());

function jsonResponse(body: unknown, status = 200) {
  return new Response(JSON.stringify(body), { status });
}

function call(fetchMock: ReturnType<typeof vi.spyOn>, i = 0): { url: string; method: string; body: unknown } {
  const [url, init] = fetchMock.mock.calls[i] as [string, RequestInit | undefined];
  return {
    url,
    method: init?.method ?? 'GET',
    body: init?.body ? JSON.parse(init.body as string) : undefined,
  };
}

const ROW: AgentiflowDefRow = {
  name: 'assess-lab',
  scope: 'lab-app',
  scope_kind: 'project',
  scope_id: 'ws_lab',
  description: 'Assess the lab app',
  lead: 'heron',
  engagement_profiles: ['web'],
  error: null,
};

describe('agentiflow definitions API', () => {
  it('lists every definition', async () => {
    const fetchMock = vi.spyOn(globalThis, 'fetch').mockResolvedValue(jsonResponse([ROW]));
    expect(await agentiflowsApi.listDefs()).toEqual([ROW]);
    expect(call(fetchMock)).toEqual({ url: '/api/agentiflow-defs', method: 'GET', body: undefined });
  });

  it('reads, saves and deletes the file a selector names', async () => {
    const fetchMock = vi
      .spyOn(globalThis, 'fetch')
      .mockImplementation(async () => jsonResponse({ deleted: true, scope: 'lab-app', scope_kind: 'project' }));
    const sel = defSelectorFor(ROW);
    await agentiflowsApi.getDef('assess-lab', sel);
    await agentiflowsApi.saveDef('assess-lab', 'name: assess-lab\n', sel);
    const deleted = await agentiflowsApi.deleteDef('assess-lab', sel);

    expect(call(fetchMock, 0)).toEqual({
      url: '/api/agentiflow-defs/assess-lab?scope_kind=project&scope_id=ws_lab',
      method: 'GET',
      body: undefined,
    });
    expect(call(fetchMock, 1)).toEqual({
      url: '/api/agentiflow-defs/assess-lab?scope_kind=project&scope_id=ws_lab',
      method: 'PUT',
      body: { raw: 'name: assess-lab\n' },
    });
    expect(call(fetchMock, 2).method).toBe('DELETE');
    expect(deleted).toEqual({ deleted: true, scope: 'lab-app', scope_kind: 'project' });
  });

  it('a global selector sends no scope_id', () => {
    expect(defSelectorFor({ ...ROW, scope_kind: 'global', scope_id: null })).toEqual({ scope_kind: 'global' });
  });

  it('creates a definition from its raw YAML', async () => {
    const fetchMock = vi.spyOn(globalThis, 'fetch').mockResolvedValue(jsonResponse({ name: 'x' }));
    await agentiflowsApi.createDef('name: x\n');
    expect(call(fetchMock)).toEqual({ url: '/api/agentiflow-defs', method: 'POST', body: { raw: 'name: x\n' } });
  });

  it('validate: ok carries the lead and profiles', async () => {
    const fetchMock = vi
      .spyOn(globalThis, 'fetch')
      .mockResolvedValue(jsonResponse({ ok: true, name: 'x', lead: 'heron', engagement_profiles: ['web'] }));
    expect(await agentiflowsApi.validateDef('name: x\n', 'ws_lab')).toEqual({
      ok: true,
      name: 'x',
      lead: 'heron',
      engagement_profiles: ['web'],
    });
    expect(call(fetchMock).body).toEqual({ raw: 'name: x\n', scope_id: 'ws_lab' });
  });

  it("validate: a 400 is an invalid draft with the server's message", async () => {
    vi.spyOn(globalThis, 'fetch').mockResolvedValue(
      jsonResponse({ error: 'unknown engagement profile `wbe`' }, 400),
    );
    expect(await agentiflowsApi.validateDef('name: x\n')).toEqual({
      ok: false,
      error: 'unknown engagement profile `wbe`',
    });
  });

  it('validate: a 500 throws rather than reading as a verdict', async () => {
    vi.spyOn(globalThis, 'fetch').mockResolvedValue(jsonResponse({ error: 'boom' }, 500));
    await expect(agentiflowsApi.validateDef('name: x\n')).rejects.toThrow();
  });
});

describe('agentiflow launch and stop', () => {
  it('launches in the layer a definition lives in', async () => {
    const fetchMock = vi.spyOn(globalThis, 'fetch').mockResolvedValue(jsonResponse({ id: 'af_1', status: 'started' }, 202));
    const res = await agentiflowsApi.launch('assess-lab', { scope_kind: 'project', scope_id: 'ws_lab' });
    expect(res).toEqual({ id: 'af_1', status: 'started' });
    expect(call(fetchMock)).toEqual({
      url: '/api/agentiflows',
      method: 'POST',
      body: { definition: 'assess-lab', scope_kind: 'project', scope_id: 'ws_lab' },
    });
  });

  it('launches a global definition with no scope_id, or in a directory', async () => {
    const fetchMock = vi
      .spyOn(globalThis, 'fetch')
      .mockImplementation(async () => jsonResponse({ id: 'af_2', status: 'started' }, 202));
    await agentiflowsApi.launch('sweep', { scope_kind: 'global', scope_id: null });
    await agentiflowsApi.launch('sweep', { working_dir: '/srv/lab' });
    expect(call(fetchMock, 0).body).toEqual({ definition: 'sweep', scope_kind: 'global' });
    expect(call(fetchMock, 1).body).toEqual({ definition: 'sweep', working_dir: '/srv/lab' });
  });

  it('stop sends now as a boolean', async () => {
    const fetchMock = vi
      .spyOn(globalThis, 'fetch')
      .mockImplementation(async () => jsonResponse({ queued: true }));
    await agentiflowsApi.stop('af_1', false);
    await agentiflowsApi.stop('af_1', true);
    expect(call(fetchMock, 0)).toEqual({ url: '/api/agentiflows/af_1/stop', method: 'POST', body: { now: false } });
    expect(call(fetchMock, 1).body).toEqual({ now: true });
  });
});
