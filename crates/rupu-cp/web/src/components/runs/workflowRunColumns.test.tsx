// @vitest-environment jsdom
// workflowRunColumns — the Workflows run-table columns shared by Runs →
// Workflows and Runs → Autoflows (CP autoflow runs as one table, 2026-10-14).
// Pins the canonical column order and the duration fallback both pages rely on.

import '@testing-library/jest-dom/vitest';
import { describe, it, expect } from 'vitest';
import { WORKFLOW_RUN_COLUMNS, runDurationMs } from './workflowRunColumns';

describe('workflowRunColumns', () => {
  it('keeps the Workflows column order', () => {
    expect(WORKFLOW_RUN_COLUMNS.map((c) => c.key)).toEqual([
      'status',
      'workflow',
      'run',
      'trigger',
      'host',
      'in',
      'out',
      'cached',
      'cost',
      'turns',
      'duration',
      'started',
    ]);
  });

  it('derives a duration from start/finish when duration_ms is absent', () => {
    expect(
      runDurationMs({
        duration_ms: null,
        started_at: '2026-10-01T10:00:00Z',
        finished_at: '2026-10-01T10:00:05Z',
      } as never),
    ).toBe(5000);
  });

  it('prefers an explicit duration_ms', () => {
    expect(
      runDurationMs({
        duration_ms: 1234,
        started_at: '2026-10-01T10:00:00Z',
        finished_at: '2026-10-01T10:00:05Z',
      } as never),
    ).toBe(1234);
  });
});
