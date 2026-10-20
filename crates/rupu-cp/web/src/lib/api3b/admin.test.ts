import { describe, it, expect, vi, afterEach } from 'vitest';
import { ApiError } from '../api';
import { admin, accountModes, loginCommand, pruneSummary, ruleLines, wakeDue } from './admin';

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

describe('autoflow operations', () => {
  it('runAutoflow posts the issue ref to /api/autoflows/:name/run', async () => {
    const reply = { started: true, name: 'issue-triage', issue: 'github:acme/widgets/issues/7', pid: 4242, log: '/tmp/x.log' };
    const fetchMock = vi.spyOn(globalThis, 'fetch').mockImplementation(async () => jsonResponse(reply, 202));

    const got = await admin.runAutoflow('issue-triage', { issue: 'github:acme/widgets/issues/7' });

    expect(call(fetchMock)).toEqual({
      url: '/api/autoflows/issue-triage/run',
      method: 'POST',
      body: { issue: 'github:acme/widgets/issues/7' },
    });
    expect(got).toEqual(reply);
  });

  it('runAutoflow surfaces the 400 for a bare issue number', async () => {
    vi.spyOn(globalThis, 'fetch').mockResolvedValue(
      jsonResponse({ error: 'issue `42` is not a full issue ref' }, 400),
    );

    await expect(admin.runAutoflow('issue-triage', { issue: '42' })).rejects.toMatchObject({ status: 400 });
  });

  it('getAutoflowExplain passes the issue ref as a query parameter', async () => {
    const fetchMock = vi
      .spyOn(globalThis, 'fetch')
      .mockImplementation(async () => jsonResponse({ kind: 'autoflow_explain', version: 1, item: { issue: 'x' } }));

    await admin.getAutoflowExplain('github:acme/widgets/issues/7');

    expect(call(fetchMock).url).toBe('/api/autoflows/explain?issue_ref=github%3Aacme%2Fwidgets%2Fissues%2F7');
  });

  it('getAutoflowDoctor and getAutoflowWakes read their reports', async () => {
    const doctor = { kind: 'autoflow_doctor', version: 1, ok: false, rows: [{ scope: 'a', problem: 'stale_lock', detail: 'd' }] };
    const wakes = { kind: 'autoflow_wakes', version: 1, rows: [] };
    const fetchMock = vi
      .spyOn(globalThis, 'fetch')
      .mockResolvedValueOnce(jsonResponse(doctor))
      .mockResolvedValueOnce(jsonResponse(wakes));

    expect(await admin.getAutoflowDoctor()).toEqual(doctor);
    expect(await admin.getAutoflowWakes()).toEqual(wakes);
    expect(call(fetchMock, 0).url).toBe('/api/autoflows/doctor');
    expect(call(fetchMock, 1).url).toBe('/api/autoflows/wakes');
  });

  it('repairClaim sends dry_run explicitly, both ways', async () => {
    const fetchMock = vi
      .spyOn(globalThis, 'fetch')
      .mockImplementation(async () => jsonResponse({ issue_ref: 'r', dry_run: true, repairs: ['would release claim'] }));

    await admin.repairClaim({ issue_ref: 'github:acme/widgets/issues/7', release: true, dry_run: true });
    await admin.repairClaim({ issue_ref: 'github:acme/widgets/issues/7', release: true, dry_run: false });

    expect(call(fetchMock, 0)).toEqual({
      url: '/api/autoflows/claims/repair',
      method: 'POST',
      body: { issue_ref: 'github:acme/widgets/issues/7', release: true, dry_run: true },
    });
    expect(call(fetchMock, 1).body).toEqual({ issue_ref: 'github:acme/widgets/issues/7', release: true, dry_run: false });
  });
});

describe('cron and maintenance', () => {
  it('cronTick dry run returns what would fire; a real tick returns started', async () => {
    const dry = { kind: 'cron_tick', dry_run: true, would_fire: [{ tier: 'cron', workflow: 'nightly-audit' }] };
    const fetchMock = vi
      .spyOn(globalThis, 'fetch')
      .mockResolvedValueOnce(jsonResponse(dry))
      .mockResolvedValueOnce(jsonResponse({ started: true, dry_run: false }, 202));

    expect(await admin.cronTick({ dry_run: true })).toEqual(dry);
    expect(await admin.cronTick({ dry_run: false })).toEqual({ started: true, dry_run: false });
    expect(call(fetchMock, 0)).toEqual({ url: '/api/cron/tick', method: 'POST', body: { dry_run: true } });
  });

  it('cronTick rejects with the 409 while another tick runs', async () => {
    vi.spyOn(globalThis, 'fetch').mockImplementation(async () => jsonResponse({ error: 'a cron tick is already running' }, 409));

    const err = await admin.cronTick({ dry_run: false }).catch((e: unknown) => e);
    expect(err).toBeInstanceOf(ApiError);
    expect((err as ApiError).status).toBe(409);
  });

  it('prune sends kind, older_than and dry_run', async () => {
    const fetchMock = vi
      .spyOn(globalThis, 'fetch')
      .mockImplementation(async () => jsonResponse({ kind: 'transcripts', dry_run: true, would_remove: [], errors: [] }));

    await admin.prune({ kind: 'transcripts', older_than: '30d', dry_run: true });
    await admin.prune({ kind: 'cleanup', dry_run: false });

    expect(call(fetchMock, 0)).toEqual({
      url: '/api/maintenance/prune',
      method: 'POST',
      body: { kind: 'transcripts', older_than: '30d', dry_run: true },
    });
    expect(call(fetchMock, 1).body).toEqual({ kind: 'cleanup', dry_run: false });
  });

  it('pruneSummary counts the rows and sums netflow bytes', () => {
    expect(pruneSummary('transcripts', [{}, {}, {}])).toBe('3 transcripts');
    expect(pruneSummary('sessions', [{}])).toBe('1 session');
    expect(pruneSummary('netflow', [{ bytes: 1_048_576 }, { bytes: 1_048_576 }])).toBe('2 ledgers · 2.0 MB');
    expect(pruneSummary('cleanup', [{}, {}])).toBe('2 items');
    expect(pruneSummary('transcripts', [])).toBe('nothing');
  });
});

describe('accounts', () => {
  it('reads auth status, SCM accounts and rules from their endpoints', async () => {
    const fetchMock = vi
      .spyOn(globalThis, 'fetch')
      .mockResolvedValueOnce(jsonResponse([]))
      .mockResolvedValueOnce(jsonResponse([]))
      .mockResolvedValueOnce(jsonResponse([]));

    await admin.getAuthStatus();
    await admin.getScmAccounts();
    await admin.getScmRules();

    expect([0, 1, 2].map((i) => call(fetchMock, i).url)).toEqual([
      '/api/auth/status',
      '/api/scm/accounts',
      '/api/scm/rules',
    ]);
  });

  it('addScmRule posts exactly the rule it was given', async () => {
    const fetchMock = vi
      .spyOn(globalThis, 'fetch')
      .mockImplementation(async () => jsonResponse({ rule: { owner: 'acme/*', account: 'gl.work' }, rules: [] }));

    await admin.addScmRule({ owner: 'acme/*', account: 'gl.work' });

    expect(call(fetchMock)).toEqual({
      url: '/api/scm/rules',
      method: 'POST',
      body: { owner: 'acme/*', account: 'gl.work' },
    });
  });

  it('accountModes names the credential kinds an account holds', () => {
    expect(accountModes({ api_key: true, sso: null })).toBe('api-key');
    expect(accountModes({ api_key: false, sso: { expires_at: null, expired: false, refreshable: true, summary: 'no expiry' } })).toBe('sso');
    expect(accountModes({ api_key: true, sso: { expires_at: null, expired: false, refreshable: false, summary: '' } })).toBe('api-key, sso');
    expect(accountModes({ api_key: false, sso: null })).toBe('—');
  });

  it('loginCommand gives the CLI line for an account, SSO when it had a token', () => {
    expect(loginCommand('gl.work', true)).toBe('rupu auth login --account gl.work --mode sso');
    expect(loginCommand('openai', false)).toBe('rupu auth login --account openai');
  });

  it('ruleLines renders each rule as its target and account', () => {
    expect(ruleLines([{ owner: 'acme/*', account: 'gl.work' }, { path: '~/Code/work/*', account: 'github' }])).toEqual([
      { target: 'owner acme/*', account: 'gl.work' },
      { target: 'path ~/Code/work/*', account: 'github' },
    ]);
  });
});

describe('tracked repos', () => {
  it('attach, prefer and forget hit their endpoints', async () => {
    const row = { id: 'github_acme_widgets', repo: 'github:acme/widgets', preferred_path: '/src/widgets', known_paths: ['/src/widgets'], last_seen_at: 't' };
    const fetchMock = vi
      .spyOn(globalThis, 'fetch')
      .mockResolvedValueOnce(jsonResponse(row))
      .mockResolvedValueOnce(jsonResponse(row))
      .mockResolvedValueOnce(jsonResponse({ repo: 'github:acme/widgets', path: '/src/w2', remaining: row }))
      .mockResolvedValueOnce(jsonResponse({ repo: 'github:acme/widgets', remaining: null }));

    await admin.attachRepo({ repo: 'github:acme/widgets', path: '/src/widgets' });
    await admin.preferRepo({ repo: 'github:acme/widgets', path: '/src/widgets' });
    await admin.forgetRepo('github_acme_widgets', '/src/w2');
    await admin.forgetRepo('github_acme_widgets');

    expect(call(fetchMock, 0)).toEqual({ url: '/api/repos/tracked', method: 'POST', body: { repo: 'github:acme/widgets', path: '/src/widgets' } });
    expect(call(fetchMock, 1).url).toBe('/api/repos/tracked/prefer');
    expect(call(fetchMock, 2)).toMatchObject({ url: '/api/repos/tracked/github_acme_widgets?path=%2Fsrc%2Fw2', method: 'DELETE' });
    expect(call(fetchMock, 3)).toMatchObject({ url: '/api/repos/tracked/github_acme_widgets', method: 'DELETE' });
  });
});

describe('wakeDue', () => {
  const now = Date.parse('2026-10-20T12:00:00Z');
  it('says due now once not_before has passed, else how long until it is', () => {
    expect(wakeDue('2026-10-20T11:00:00Z', now)).toBe('due now');
    expect(wakeDue('2026-10-20T12:20:00Z', now)).toBe('due in 20m');
    expect(wakeDue('2026-10-20T15:00:00Z', now)).toBe('due in 3h');
    expect(wakeDue('2026-10-23T12:00:00Z', now)).toBe('due in 3d');
  });
});
