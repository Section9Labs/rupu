/**
 * Unit tests for the rupu API client.
 * Tests the `request` wrapper via the exported `api` object.
 * EventSource subscribe helpers are not covered here (fiddly to mock in jsdom).
 */

import { describe, it, expect, vi, afterEach } from 'vitest';
import { api, ApiError } from './api';

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function mockFetch(status: number, body: unknown): void {
  const text = typeof body === 'string' ? body : JSON.stringify(body);
  vi.stubGlobal(
    'fetch',
    vi.fn().mockResolvedValue({
      ok: status >= 200 && status < 300,
      status,
      statusText: status === 200 ? 'OK' : 'Not Found',
      text: () => Promise.resolve(text),
    }),
  );
}

afterEach(() => {
  vi.unstubAllGlobals();
});

// ---------------------------------------------------------------------------
// request wrapper — success path
// ---------------------------------------------------------------------------

describe('api.getDashboard', () => {
  it('resolves typed on 200 JSON', async () => {
    const payload = {
      runs: { total: 3, by_status: { pending: 1, running: 1, completed: 1, failed: 0, awaiting_approval: 0, rejected: 0 } },
      recent_runs: [{ id: 'r1', workflow_name: 'wf', status: 'running', started_at: '2026-01-01T00:00:00Z' }],
      sessions: { total: 2, active: 1, archived: 1 },
      workers: { total: 1 },
      coverage: { targets: 5, assertions: 42 },
    };
    mockFetch(200, payload);

    const result = await api.getDashboard();
    expect(result.runs.total).toBe(3);
    expect(result.recent_runs[0].id).toBe('r1');
    expect(result.workers.total).toBe(1);
  });
});

describe('api.getRuns', () => {
  it('returns an array of RunRecord on 200', async () => {
    const runs = [
      { id: 'abc', workflow_name: 'demo', status: 'completed', started_at: '2026-01-01T00:00:00Z' },
    ];
    mockFetch(200, runs);

    const result = await api.getRuns();
    expect(result).toHaveLength(1);
    expect(result[0].id).toBe('abc');
    expect(result[0].status).toBe('completed');
  });
});

describe('api.getRun', () => {
  it('resolves run + steps on 200', async () => {
    const payload = {
      run: { id: 'r1', workflow_name: 'wf', status: 'running', started_at: '2026-01-01T00:00:00Z' },
      steps: [{ run_id: 'r1', step_id: 'classify', success: true }],
    };
    mockFetch(200, payload);

    const result = await api.getRun('r1');
    expect(result.run.id).toBe('r1');
    expect(result.steps[0].step_id).toBe('classify');
  });
});

describe('api.getUsage', () => {
  it('requests /api/usage?group_by=model when grouped by model', async () => {
    mockFetch(200, { summary: { input_tokens: 0, output_tokens: 0, cached_tokens: 0, total_tokens: 0, cost_usd: null, priced: false, runs: 0 }, breakdown: [] });
    const fetchSpy = vi.mocked(fetch);
    await api.getUsage({ groupBy: 'model' });
    const calledUrl = (fetchSpy.mock.calls[0][0] as string);
    expect(calledUrl).toBe('/api/usage?group_by=model');
  });
});

// ---------------------------------------------------------------------------
// request wrapper — error path
// ---------------------------------------------------------------------------

describe('ApiError', () => {
  it('is thrown with status 404 on a not-found response', async () => {
    mockFetch(404, 'not found');

    await expect(api.getRuns()).rejects.toThrow(ApiError);
    await expect(api.getRuns()).rejects.toMatchObject({ status: 404 });
  });

  it('is thrown with status 500 on server error', async () => {
    mockFetch(500, 'internal server error');

    await expect(api.getDashboard()).rejects.toThrow(ApiError);
    await expect(api.getDashboard()).rejects.toMatchObject({ status: 500 });
  });

  it('carries the response body text', async () => {
    mockFetch(422, 'invalid input');

    try {
      await api.getAgents();
    } catch (e) {
      expect(e).toBeInstanceOf(ApiError);
      expect((e as ApiError).body).toBe('invalid input');
    }
  });
});

// ---------------------------------------------------------------------------
// 204 No Content → undefined (not a parse error)
// ---------------------------------------------------------------------------

describe('204 No Content', () => {
  it('resolves to undefined without throwing', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn().mockResolvedValue({
        ok: true,
        status: 204,
        statusText: 'No Content',
        text: () => Promise.resolve(''),
      }),
    );

    // Use getWorkers() as a convenient typed call; result will be undefined
    const result = await api.getWorkers();
    expect(result).toBeUndefined();
  });
});

// ---------------------------------------------------------------------------
// URL encoding — paths with slashes / special chars
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// New run-graph + run-stream endpoints
// ---------------------------------------------------------------------------

describe('api.getRunGraph', () => {
  it('resolves typed RunGraphResponse on 200', async () => {
    const payload = {
      run: { id: 'run-1', workflow_name: 'audit', status: 'completed', started_at: '2026-01-01T00:00:00Z' },
      workflow: {
        steps: [
          { id: 'classify', kind: 'step', agent: 'classifier', for_each: null, parallel: null, panelists: null, gate: null },
          {
            id: 'fix-loop', kind: 'for_each', agent: null, for_each: '{{ findings }}', parallel: null, panelists: null,
            gate: { max_iterations: 3, until_severity: 'low', fix_with: 'fixer' },
          },
        ],
      },
      step_results: [{ run_id: 'run-1', step_id: 'classify', success: true }],
      units: [
        { step_id: 'fix-loop', index: 0, item: 'CVE-2025-001', success: true, finished_at: '2026-01-01T00:01:00Z' },
      ],
    };
    mockFetch(200, payload);

    const result = await api.getRunGraph('run-1');
    expect(result.run.id).toBe('run-1');
    expect(result.workflow.steps).toHaveLength(2);
    expect(result.workflow.steps[0].kind).toBe('step');
    expect(result.workflow.steps[1].gate?.until_severity).toBe('low');
    expect(result.step_results[0].step_id).toBe('classify');
    expect(result.units[0].item).toBe('CVE-2025-001');
    expect(result.units[0].success).toBe(true);
  });

  it('throws ApiError on 404', async () => {
    mockFetch(404, 'run not found');
    await expect(api.getRunGraph('no-such-run')).rejects.toThrow(ApiError);
    await expect(api.getRunGraph('no-such-run')).rejects.toMatchObject({ status: 404 });
  });
});

describe('api.getAgentRuns', () => {
  it('resolves typed AgentRunRow[] on 200', async () => {
    const payload = [
      {
        run_id: 'ar-1', source: 'standalone', agent: 'classifier',
        session_id: null, trigger_source: null, status: 'completed',
        started_at: '2026-01-01T00:00:00Z', transcript_path: '/tmp/t.jsonl',
      },
    ];
    mockFetch(200, payload);

    const result = await api.getAgentRuns();
    expect(result).toHaveLength(1);
    expect(result[0].run_id).toBe('ar-1');
    expect(result[0].source).toBe('standalone');
    expect(result[0].agent).toBe('classifier');
  });
});

describe('api.getWorkflowRuns', () => {
  it('resolves typed RunListRow[] on 200', async () => {
    const payload = [
      { id: 'wr-1', workflow_name: 'nightly', status: 'completed', started_at: '2026-01-01T00:00:00Z', trigger: 'cron' },
    ];
    mockFetch(200, payload);

    const result = await api.getWorkflowRuns();
    expect(result).toHaveLength(1);
    expect(result[0].trigger).toBe('cron');
    expect(result[0].workflow_name).toBe('nightly');
  });
});

describe('api.getAutoflowRuns', () => {
  it('resolves typed AutoflowCycleRow[] on 200', async () => {
    const payload = [
      {
        cycle_id: 'cyc-1', mode: 'auto', worker_name: 'worker-a',
        started_at: '2026-01-01T00:00:00Z', finished_at: '2026-01-01T00:05:00Z',
        workflow_count: 3, ran_cycles: 2, skipped_cycles: 1, failed_cycles: 0,
        run_ids: ['r1', 'r2'],
      },
    ];
    mockFetch(200, payload);

    const result = await api.getAutoflowRuns();
    expect(result).toHaveLength(1);
    expect(result[0].cycle_id).toBe('cyc-1');
    expect(result[0].ran_cycles).toBe(2);
    expect(result[0].run_ids).toEqual(['r1', 'r2']);
  });
});

describe('api.getAutoflowDefs', () => {
  it('resolves typed AutoflowDefRow[] on 200', async () => {
    const payload = [
      { name: 'nightly-audit', trigger: 'cron(0 2 * * *)', scope: 'workspace' },
    ];
    mockFetch(200, payload);

    const result = await api.getAutoflowDefs();
    expect(result).toHaveLength(1);
    expect(result[0].name).toBe('nightly-audit');
    expect(result[0].trigger).toBe('cron(0 2 * * *)');
  });
});

// ---------------------------------------------------------------------------
// Projects
// ---------------------------------------------------------------------------

describe('api.getProjects', () => {
  it('resolves typed ProjectRow[] on 200', async () => {
    const payload = [
      {
        ws_id: 'ws-1', name: 'rupu', path: '/code/rupu',
        repo_remote: 'git@github.com:org/rupu.git', branch: 'main',
        created_at: '2026-01-01T00:00:00Z', last_run_at: '2026-06-01T00:00:00Z',
      },
    ];
    mockFetch(200, payload);

    const result = await api.getProjects();
    expect(result).toHaveLength(1);
    expect(result[0].ws_id).toBe('ws-1');
    expect(result[0].name).toBe('rupu');
    expect(result[0].branch).toBe('main');
  });
});

describe('api.getProject', () => {
  it('resolves typed ProjectDetail on 200 (no assessed_pct in coverage rollup)', async () => {
    // The synchronous rollup no longer carries assessed_pct — only targets +
    // findings.  assessed_pct is fetched lazily via getProjectAssessedPct.
    const payload = {
      project: {
        ws_id: 'ws-1', name: 'rupu', path: '/code/rupu',
        repo_remote: null, branch: null,
        created_at: '2026-01-01T00:00:00Z', last_run_at: null,
      },
      runs: {
        total: 10, running: 1,
        by_status: { completed: 8, failed: 1, running: 1 },
        by_surface: { workflow: 7, autoflow: 3 },
      },
      sessions: { total: 5, active: 2 },
      coverage: { targets: 12, findings: 3 },
      recent_runs: [
        { id: 'r1', workflow_name: 'audit', status: 'completed', started_at: '2026-06-01T00:00:00Z', trigger: 'manual' },
      ],
    };
    mockFetch(200, payload);

    const result = await api.getProject('ws-1');
    expect(result.project.ws_id).toBe('ws-1');
    expect(result.runs.total).toBe(10);
    expect(result.runs.by_surface.workflow).toBe(7);
    expect(result.sessions.active).toBe(2);
    expect(result.coverage.targets).toBe(12);
    expect(result.coverage.findings).toBe(3);
    expect(result.recent_runs[0].id).toBe('r1');
  });

  it('throws ApiError on 404', async () => {
    mockFetch(404, 'project not found');
    await expect(api.getProject('no-such-ws')).rejects.toThrow(ApiError);
    await expect(api.getProject('no-such-ws')).rejects.toMatchObject({ status: 404 });
  });
});

describe('api.getProjectRuns', () => {
  it('resolves typed RunListRow[] on 200', async () => {
    const payload = [
      { id: 'r2', workflow_name: 'nightly', status: 'completed', started_at: '2026-06-02T00:00:00Z', trigger: 'cron' },
    ];
    mockFetch(200, payload);

    const result = await api.getProjectRuns('ws-1');
    expect(result).toHaveLength(1);
    expect(result[0].trigger).toBe('cron');
  });
});

describe('api.getProjectCoverage', () => {
  it('resolves typed ProjectCoverageRow[] on 200', async () => {
    const payload = [
      { target_id: 'auth.rs', assertion_lines: 42, has_catalog: true, findings: 2 },
    ];
    mockFetch(200, payload);

    const result = await api.getProjectCoverage('ws-1');
    expect(result).toHaveLength(1);
    expect(result[0].target_id).toBe('auth.rs');
    expect(result[0].has_catalog).toBe(true);
    expect(result[0].findings).toBe(2);
  });
});

describe('api.getProjectAssessedPct', () => {
  it('resolves { assessed_pct: number } when catalog is present', async () => {
    mockFetch(200, { assessed_pct: 87.5 });

    const result = await api.getProjectAssessedPct('ws-1');
    expect(result.assessed_pct).toBe(87.5);
  });

  it('resolves { assessed_pct: null } when no catalog is present', async () => {
    mockFetch(200, { assessed_pct: null });

    const result = await api.getProjectAssessedPct('ws-1');
    expect(result.assessed_pct).toBeNull();
  });

  it('throws ApiError on 404 (unknown project)', async () => {
    mockFetch(404, 'project not found');
    await expect(api.getProjectAssessedPct('no-such-ws')).rejects.toThrow(ApiError);
    await expect(api.getProjectAssessedPct('no-such-ws')).rejects.toMatchObject({ status: 404 });
  });
});

// ---------------------------------------------------------------------------
// Transcripts
// ---------------------------------------------------------------------------

describe('api.getTranscript', () => {
  it('resolves typed TranscriptResponse on 200', async () => {
    const payload = {
      events: [
        { type: 'run_start', data: { run_id: 'r1', agent: 'classifier', provider: 'anthropic', model: 'claude-3', started_at: '2026-01-01T00:00:00Z', mode: 'bypass' } },
        { type: 'assistant_message', data: { content: 'Hello!', thinking: null } },
        { type: 'tool_call', data: { call_id: 'c1', tool: 'read_file', input: { path: '/foo.rs' } } },
        { type: 'tool_result', data: { call_id: 'c1', output: 'fn main() {}', error: null, duration_ms: 5 } },
        { type: 'run_complete', data: { run_id: 'r1', status: 'completed', total_tokens: 1000, duration_ms: 1234, error: null } },
      ],
      summary: {
        run_id: 'r1', agent: 'classifier', provider: 'anthropic', model: 'claude-3',
        status: 'completed', total_tokens: 1000, duration_ms: 1234,
        started_at: '2026-01-01T00:00:00Z', error: null,
      },
    };
    mockFetch(200, payload);

    const result = await api.getTranscript('/tmp/run-1/transcript.jsonl');
    expect(result.events).toHaveLength(5);
    expect(result.events[0].type).toBe('run_start');
    // Narrow to run_start to verify data shape
    const first = result.events[0];
    if (first.type === 'run_start') {
      expect(first.data.run_id).toBe('r1');
      expect(first.data.agent).toBe('classifier');
      expect(first.data.mode).toBe('bypass');
    } else {
      throw new Error('expected run_start');
    }
    // Narrow tool_call
    const toolCall = result.events[2];
    if (toolCall.type === 'tool_call') {
      expect(toolCall.data.call_id).toBe('c1');
      expect(toolCall.data.tool).toBe('read_file');
    } else {
      throw new Error('expected tool_call');
    }
    // Summary
    expect(result.summary).not.toBeNull();
    expect(result.summary?.run_id).toBe('r1');
    expect(result.summary?.total_tokens).toBe(1000);
  });

  it('handles null summary', async () => {
    mockFetch(200, { events: [], summary: null });
    const result = await api.getTranscript('/tmp/run-2/transcript.jsonl');
    expect(result.events).toHaveLength(0);
    expect(result.summary).toBeNull();
  });

  it('throws ApiError on 404', async () => {
    mockFetch(404, 'transcript not found');
    await expect(api.getTranscript('/no/such/path.jsonl')).rejects.toThrow(ApiError);
    await expect(api.getTranscript('/no/such/path.jsonl')).rejects.toMatchObject({ status: 404 });
  });
});

// ---------------------------------------------------------------------------
// URL encoding — paths with slashes / special chars
// ---------------------------------------------------------------------------

describe('encodeURIComponent in paths', () => {
  it('encodes run id in getRun', async () => {
    mockFetch(200, { run: { id: 'r/1', workflow_name: 'wf', status: 'pending', started_at: '' }, steps: [] });
    const fetchSpy = vi.mocked(fetch);
    await api.getRun('r/1');
    const calledUrl = (fetchSpy.mock.calls[0][0] as string);
    expect(calledUrl).toBe('/api/runs/r%2F1');
  });

  it('encodes coverage target id in getCoverageDetail', async () => {
    mockFetch(200, { target_id: 'foo/bar', assertions: [], findings: [] });
    const fetchSpy = vi.mocked(fetch);
    await api.getCoverageDetail('foo/bar');
    const calledUrl = (fetchSpy.mock.calls[0][0] as string);
    expect(calledUrl).toBe('/api/coverage/foo%2Fbar');
  });
});
