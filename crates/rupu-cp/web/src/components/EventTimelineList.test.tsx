// visualFor — the icon/color a timeline row gets per event type. The key
// regression guarded here: unit_started and unit_completed must render
// DIFFERENTLY (they used to share one gray circle, so a finished unit looked
// identical to a just-started one — completion was invisible).

import { describe, it, expect } from 'vitest';
import { CheckCircle2, PlayCircle, XCircle } from 'lucide-react';
import { visualFor } from './EventTimelineList';
import type { UnitStartedEvent, UnitCompletedEvent } from '../lib/api';

const started: UnitStartedEvent = {
  type: 'unit_started',
  run_id: 'run_1',
  step_id: 'review',
  index: 0,
  unit_key: 'a.rs',
  transcript_path: '/tmp/a.jsonl',
};

const ok: UnitCompletedEvent = {
  type: 'unit_completed',
  run_id: 'run_1',
  step_id: 'review',
  index: 0,
  unit_key: 'a.rs',
  success: true,
  tokens_in: 10,
  tokens_out: 20,
};

const failed: UnitCompletedEvent = { ...ok, success: false };

describe('visualFor — unit lifecycle is visually distinct', () => {
  it('unit_started uses a play icon (in-progress), not the same as completion', () => {
    expect(visualFor(started).icon).toBe(PlayCircle);
  });

  it('a successful unit_completed shows a green check', () => {
    const v = visualFor(ok);
    expect(v.icon).toBe(CheckCircle2);
    expect(v.iconColor).toContain('ok');
  });

  it('a failed unit_completed shows a red X', () => {
    const v = visualFor(failed);
    expect(v.icon).toBe(XCircle);
    expect(v.iconColor).toContain('err');
  });

  it('started and completed never resolve to the same icon', () => {
    expect(visualFor(started).icon).not.toBe(visualFor(ok).icon);
  });
});
