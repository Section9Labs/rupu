import { describe, it, expect, vi, afterEach } from 'vitest';
import {
  fleetWriteCount,
  getEngagementProfiles,
  getFleet,
  initProject,
  installFleet,
  launchAgentRun,
  launchFieldGates,
  launchWorkflowRun,
  profileSummary,
  runWorkflowYaml,
  validateAgentRaw,
  validateAllAgents,
  type EngagementProfile,
} from './launch';

afterEach(() => vi.restoreAllMocks());

function mockFetch(body: unknown, status = 200) {
  return vi
    .spyOn(globalThis, 'fetch')
    .mockImplementation(async () => new Response(typeof body === 'string' ? body : JSON.stringify(body), { status }));
}

function call(m: ReturnType<typeof mockFetch>, i = 0): { url: string; method?: string; body?: unknown } {
  const [url, init] = m.mock.calls[i];
  return {
    url: String(url),
    method: init?.method,
    body: init?.body ? JSON.parse(String(init.body)) : undefined,
  };
}

const NETWORK: EngagementProfile = {
  id: 'network',
  name: 'Network assessment',
  layer: 'builtin',
  includes: [],
  asset_kinds: [
    { id: 'host', label: '{host}' },
    { id: 'port', label: '{host}:{port}' },
  ],
  bundle: { agents: ['recon', 'service-analyst'], tools: [], workflows: ['network-assessment'] },
};

describe('engagement profiles', () => {
  it('GETs the list, with the project overlay when given', async () => {
    const m = mockFetch({ profiles: [], errors: [] });
    await getEngagementProfiles();
    await getEngagementProfiles('ws 1');
    expect(call(m, 0).url).toBe('/api/engagement-profiles');
    expect(call(m, 1).url).toBe('/api/engagement-profiles?ws_id=ws%201');
  });

  it('summarises a profile as its bundle and asset kinds', () => {
    expect(profileSummary(NETWORK)).toBe('recon, service-analyst, network-assessment · asset kinds host, port');
  });

  it('summarises a composite by what it includes', () => {
    expect(
      profileSummary({ ...NETWORK, bundle: { agents: [], tools: [], workflows: [] }, includes: ['web', 'network'], asset_kinds: [] }),
    ).toBe('includes web, network');
  });
});

describe('launches', () => {
  it('POSTs an agent run with model, provider and profiles', async () => {
    const m = mockFetch({ run_id: 'run_1' });
    await launchAgentRun('code reviewer', {
      prompt: 'go',
      mode: 'ask',
      model: 'm1',
      provider: 'p1',
      engagement_profiles: ['network'],
    });
    expect(call(m)).toEqual({
      url: '/api/agents/code%20reviewer/run',
      method: 'POST',
      body: { prompt: 'go', mode: 'ask', model: 'm1', provider: 'p1', engagement_profiles: ['network'] },
    });
  });

  it('POSTs a workflow run with profiles', async () => {
    const m = mockFetch({ run_id: 'run_2' });
    await launchWorkflowRun('wf', { mode: 'bypass', engagement_profiles: ['web'] });
    expect(call(m)).toEqual({
      url: '/api/workflows/wf/run',
      method: 'POST',
      body: { mode: 'bypass', engagement_profiles: ['web'] },
    });
  });

  it('POSTs a YAML launch to /api/workflows/run', async () => {
    const m = mockFetch({ run_id: 'run_3', host_id: 'local', file: '/x.yaml' });
    const r = await runWorkflowYaml({ yaml: 'name: x\n', mode: 'ask', inputs: { a: '1' } });
    expect(r.run_id).toBe('run_3');
    expect(call(m)).toEqual({
      url: '/api/workflows/run',
      method: 'POST',
      body: { yaml: 'name: x\n', mode: 'ask', inputs: { a: '1' } },
    });
  });

  it('gates model/provider to local agent runs and profiles off remote workflows and sessions', () => {
    expect(launchFieldGates({ kind: 'agent', host: 'local' })).toEqual({ model: null, profiles: null });
    const remote = launchFieldGates({ kind: 'agent', host: 'mini' });
    expect(remote.model).toMatch(/Local host only/);
    expect(remote.profiles).toBeNull();
    const session = launchFieldGates({ kind: 'agent', host: 'local', session: true });
    expect(session.model).toMatch(/session/);
    expect(session.profiles).toMatch(/session/);
    expect(launchFieldGates({ kind: 'workflow', host: 'local' }).profiles).toBeNull();
    expect(launchFieldGates({ kind: 'workflow', host: 'mini' }).profiles).toMatch(/Local host only/);
  });
});

describe('agent validation', () => {
  it('answers ok from a 200', async () => {
    const m = mockFetch({ ok: true, name: 'a' });
    expect(await validateAgentRaw('---\nname: a\n---\n', 'ws1')).toEqual({ ok: true, name: 'a' });
    expect(call(m)).toEqual({
      url: '/api/agents/validate',
      method: 'POST',
      body: { raw: '---\nname: a\n---\n', ws_id: 'ws1' },
    });
  });

  it('answers invalid with the server message from a 400', async () => {
    mockFetch({ error: 'tools: unknown tool `bsh` (did you mean `bash`?)' }, 400);
    expect(await validateAgentRaw('x')).toEqual({
      ok: false,
      error: 'tools: unknown tool `bsh` (did you mean `bash`?)',
    });
  });

  it('throws on anything but a 400', async () => {
    mockFetch({ error: 'project not found' }, 404);
    await expect(validateAgentRaw('x', 'nope')).rejects.toThrow();
  });

  it('GETs every file’s result', async () => {
    const m = mockFetch({ ok: true, rows: [] });
    await validateAllAgents();
    await validateAllAgents('w2');
    expect(call(m, 0).url).toBe('/api/agents/validate');
    expect(call(m, 1).url).toBe('/api/agents/validate?ws_id=w2');
  });
});

describe('fleet and init', () => {
  it('GETs the fleet listing, per project when given', async () => {
    const m = mockFetch({ entries: [], global_root: '/g', project_root: null });
    await getFleet();
    await getFleet('w3');
    expect(call(m, 0).url).toBe('/api/fleet');
    expect(call(m, 1).url).toBe('/api/fleet?ws_id=w3');
  });

  it('POSTs an install', async () => {
    const m = mockFetch({ root: '/g', created: 1, skipped: 0, overwrote: 0, files: [] });
    await installFleet({ force: true, project: 'w3' });
    expect(call(m)).toEqual({ url: '/api/fleet/install', method: 'POST', body: { force: true, project: 'w3' } });
  });

  it('POSTs a project init', async () => {
    const m = mockFetch({ root: '/p', created: 2, skipped: 0, overwrote: 0, files: [] });
    await initProject({ path: '/p', with_samples: true });
    expect(call(m)).toEqual({ url: '/api/projects/init', method: 'POST', body: { path: '/p', with_samples: true } });
  });

  it('counts what an install would write', () => {
    const s = ['missing', 'current', 'modified', 'unreadable', 'missing'] as const;
    expect(fleetWriteCount([...s], false)).toBe(2);
    expect(fleetWriteCount([...s], true)).toBe(4);
  });
});
