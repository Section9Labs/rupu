import { describe, it, expect, vi, afterEach } from 'vitest';
import { api } from './api';

afterEach(() => vi.restoreAllMocks());

const PDF_ROW = {
  name: 'pdf',
  layer: 'global',
  status: 'ok',
  description: 'Read and fill PDF forms',
  error: null,
  warnings: [],
  stamp: 'ab12cd34',
  source: null,
  both_layers: false,
};

function jsonResponse(body: unknown, status = 200) {
  return new Response(JSON.stringify(body), { status });
}

function sentBody(fetchMock: ReturnType<typeof vi.spyOn>): Record<string, unknown> {
  const init = fetchMock.mock.calls[0][1] as RequestInit;
  return JSON.parse(init.body as string) as Record<string, unknown>;
}

describe('skills API', () => {
  it('getSkills lists the global layer from /api/skills', async () => {
    const fetchMock = vi.spyOn(globalThis, 'fetch').mockResolvedValue(jsonResponse({ skills: [PDF_ROW] }));

    const skills = await api.getSkills();

    expect(fetchMock.mock.calls[0][0]).toBe('/api/skills');
    expect(skills).toEqual([PDF_ROW]);
  });

  it('getSkills scopes to a project with ?ws_id=', async () => {
    const projectRow = { ...PDF_ROW, layer: 'project', status: 'invalid', error: 'name mismatch' };
    const fetchMock = vi
      .spyOn(globalThis, 'fetch')
      .mockResolvedValue(jsonResponse({ skills: [PDF_ROW, projectRow] }));

    const skills = await api.getSkills('w1');

    expect(fetchMock.mock.calls[0][0]).toBe('/api/skills?ws_id=w1');
    expect(skills.map((s) => [s.layer, s.status, s.error])).toEqual([
      ['global', 'ok', null],
      ['project', 'invalid', 'name mismatch'],
    ]);
  });

  it('getSkill fetches one entry by layer and name', async () => {
    const fetchMock = vi
      .spyOn(globalThis, 'fetch')
      .mockResolvedValue(jsonResponse({ ...PDF_ROW, layer: 'project', path: '/p/.rupu/skills/pdf', body: '# PDF' }));

    const skill = await api.getSkill('project', 'pdf', 'w1');

    expect(fetchMock.mock.calls[0][0]).toBe('/api/skills/project/pdf?ws_id=w1');
    expect(skill.body).toBe('# PDF');
  });

  it('getSkill without a project omits ws_id', async () => {
    const fetchMock = vi.spyOn(globalThis, 'fetch').mockResolvedValue(jsonResponse({ ...PDF_ROW, path: '/g/pdf' }));

    await api.getSkill('global', 'pdf');

    expect(fetchMock.mock.calls[0][0]).toBe('/api/skills/global/pdf');
  });
});

describe('skills on launches and sends', () => {
  it('launchAgent sends skills and preload_skills', async () => {
    const fetchMock = vi.spyOn(globalThis, 'fetch').mockResolvedValue(jsonResponse({ run_id: 'r1' }));

    await api.launchAgent('a', { prompt: 'go', skills: ['pdf', 'project:*'], preload_skills: ['house-style'] });

    expect(sentBody(fetchMock)).toEqual({
      prompt: 'go',
      skills: ['pdf', 'project:*'],
      preload_skills: ['house-style'],
    });
  });

  it('startSession sends skills and preload_skills', async () => {
    const fetchMock = vi.spyOn(globalThis, 'fetch').mockResolvedValue(jsonResponse({ session_id: 's1' }));

    await api.startSession('a', { skills: ['pdf'], preload_skills: ['house-style'] });

    expect(sentBody(fetchMock)).toEqual({ skills: ['pdf'], preload_skills: ['house-style'] });
  });

  it('a launch without skills posts no skills keys', async () => {
    const fetchMock = vi.spyOn(globalThis, 'fetch').mockResolvedValue(jsonResponse({ run_id: 'r1' }));

    await api.launchAgent('a', { prompt: 'go', skills: [], preload_skills: [] });

    expect(sentBody(fetchMock)).toEqual({ prompt: 'go' });
  });

  it('sendSessionMessage sends a skill-only turn with an empty prompt', async () => {
    const fetchMock = vi.spyOn(globalThis, 'fetch').mockResolvedValue(jsonResponse({ run_id: 'r2' }));

    await api.sendSessionMessage('s1', '', 'h1', ['pdf']);

    expect(fetchMock.mock.calls[0][0]).toBe('/api/sessions/s1/send?host=h1');
    expect(sentBody(fetchMock)).toEqual({ prompt: '', skills: ['pdf'] });
  });

  it('sendSessionMessage without skills posts only the prompt', async () => {
    const fetchMock = vi.spyOn(globalThis, 'fetch').mockResolvedValue(jsonResponse({ run_id: 'r2' }));

    await api.sendSessionMessage('s1', 'hello');

    expect(sentBody(fetchMock)).toEqual({ prompt: 'hello' });
  });
});
