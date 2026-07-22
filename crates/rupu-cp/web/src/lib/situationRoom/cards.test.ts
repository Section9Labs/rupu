// cardFromEvent / cardFromFinding — the pure mappers that turn REAL wire
// objects into StreamCards. These guard the information design: a finding
// keeps its severity + code excerpt; an awaiting step is approvable; an error
// lands in the error group; a note-less heartbeat is dropped (not rendered as
// an empty row).

import { describe, it, expect } from 'vitest';
import { cardFromEvent, cardFromFinding } from './cards';
import type {
  FindingOut,
  StepAwaitingApprovalEvent,
  StepFailedEvent,
  StepStartedEvent,
  StepWorkingEvent,
  PanelRoundEvent,
} from '../api';

describe('cardFromEvent', () => {
  it('an awaiting-approval step is an approvable await card', () => {
    const ev: StepAwaitingApprovalEvent = {
      type: 'step_awaiting_approval', run_id: 'r1', step_id: 'deploy', reason: 'ship it?',
    };
    const c = cardFromEvent(ev, 1000, 'k1')!;
    expect(c.group).toBe('await');
    expect(c.accent).toBe('await');
    expect(c.approvable).toEqual({ runId: 'r1', stepId: 'deploy', reason: 'ship it?' });
    expect(c.detail).toBe('ship it?');
  });

  it('a step failure is an error-group card carrying the error text', () => {
    const ev: StepFailedEvent = { type: 'step_failed', run_id: 'r1', step_id: 'checkout', error: 'clone timed out' };
    const c = cardFromEvent(ev, 1000, 'k1')!;
    expect(c.group).toBe('error');
    expect(c.accent).toBe('error');
    expect(c.detail).toBe('clone timed out');
  });

  it('an agent step_started is a Scanning activity card attributed to the agent', () => {
    const ev: StepStartedEvent = { type: 'step_started', run_id: 'r1', step_id: 'audit', kind: 'agent', agent: 'oracle-sec' };
    const c = cardFromEvent(ev, 1000, 'k1')!;
    expect(c.group).toBe('activity');
    expect(c.badge).toBe('Scanning');
    expect(c.agent).toBe('oracle-sec');
    expect(c.title).toContain('oracle-sec');
  });

  it('a note-less step_working heartbeat is dropped (null), not an empty row', () => {
    const ev: StepWorkingEvent = { type: 'step_working', run_id: 'r1', step_id: 'audit', note: null };
    expect(cardFromEvent(ev, 1000, 'k1')).toBeNull();
  });

  it('a step_working WITH a note renders the note as detail', () => {
    const ev: StepWorkingEvent = { type: 'step_working', run_id: 'r1', step_id: 'audit', note: 'reading routes' };
    const c = cardFromEvent(ev, 1000, 'k1')!;
    expect(c.detail).toBe('reading routes');
  });

  it('a panel_round surfaces the round counter and max severity remaining', () => {
    const ev: PanelRoundEvent = {
      type: 'panel_round', run_id: 'r1', step_id: 'panel', round: 2, max_iterations: 4, max_severity_remaining: 'high',
    };
    const c = cardFromEvent(ev, 1000, 'k1')!;
    expect(c.form).toBe('panel');
    expect(c.title).toContain('round 2/4');
    expect(c.detail).toContain('high');
  });
});

describe('cardFromFinding', () => {
  const base: FindingOut = {
    id: 'f-1', ws_id: 'ws1', project: 'billing-api', target_id: 't1',
    file_path: 'src/routes/billing.ts', line_range: [16, 21], scope: null,
    summary: 'Broken org-scoping on GET /invoice/:id', severity: 'HIGH', concern_id: null,
    evidence: { rationale: 'orgId is checked for truthiness, not against the caller', code_excerpt: 'if (invoice.orgId) {' },
    declared_by: null, declared_at: '2026-07-21T10:00:00Z',
  };

  it('normalizes severity, builds a file:line ref, and keeps the real code excerpt', () => {
    const c = cardFromFinding(base);
    expect(c.group).toBe('finding');
    expect(c.severity).toBe('high');
    expect(c.accent).toBe('high');
    expect(c.badge).toBe('High');
    expect(c.fileRef).toBe('src/routes/billing.ts:16-21');
    expect(c.code).toBe('if (invoice.orgId) {');
    expect(c.detail).toContain('truthiness');
    expect(c.ts).toBe(Date.parse('2026-07-21T10:00:00Z'));
  });

  it('an unknown severity falls back to info, and a missing file has no ref', () => {
    const c = cardFromFinding({ ...base, severity: 'bogus', file_path: null, line_range: null });
    expect(c.severity).toBe('info');
    expect(c.fileRef).toBeUndefined();
  });
});
