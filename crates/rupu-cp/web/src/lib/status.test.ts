import { describe, expect, it } from 'vitest';
import {
  STATUS,
  normalizeStatusKey,
  runStatusStyle,
  stepStateStyle,
} from './status';

describe('status descriptors', () => {
  it('run statuses resolve to the unified pill palette', () => {
    expect(runStatusStyle('running').hex).toBe('#3b82f6');
    expect(runStatusStyle('running').label).toBe('Running');
    expect(runStatusStyle('completed').hex).toBe('#22c55e');
    expect(runStatusStyle('completed').label).toBe('Completed');
    expect(runStatusStyle('failed').hex).toBe('#ef4444');
    expect(runStatusStyle('awaiting_approval').hex).toBe('#f59e0b');
    expect(runStatusStyle('pending').hex).toBe('#94a3b8');
    expect(runStatusStyle('cancelled').hex).toBe('#64748b');
    expect(runStatusStyle('rejected').hex).toBe('#ef4444');
  });

  it('exposes dot + pill Tailwind classes', () => {
    expect(runStatusStyle('running').dotClass).toBe('bg-status-running');
    expect(runStatusStyle('completed').dotClass).toBe('bg-status-done');
    expect(runStatusStyle('running').pillClass).toContain('text-status-running');
  });

  it('done ↔ completed alias resolves to the same descriptor', () => {
    expect(normalizeStatusKey('done')).toBe('completed');
    expect(stepStateStyle('done')).toBe(STATUS.completed);
    expect(stepStateStyle('completed')).toBe(STATUS.completed);
    expect(stepStateStyle('done').hex).toBe('#22c55e');
    expect(stepStateStyle('done').label).toBe('Completed');
  });

  it('step states resolve to the unified palette', () => {
    expect(stepStateStyle('running').hex).toBe('#3b82f6');
    expect(stepStateStyle('failed').hex).toBe('#ef4444');
    expect(stepStateStyle('awaiting_approval').hex).toBe('#f59e0b');
    expect(stepStateStyle('skipped').hex).toBe('#cbd5e1');
    expect(stepStateStyle('pending').hex).toBe('#94a3b8');
  });

  it('every descriptor carries a label, hex, tint, icon, dotClass, pillClass', () => {
    for (const d of Object.values(STATUS)) {
      expect(d.label).toBeTruthy();
      expect(d.hex).toMatch(/^#[0-9a-f]{6}$/i);
      expect(d.tint).toMatch(/^#[0-9a-f]{6}$/i);
      expect(d.icon).toBeTruthy();
      expect(d.dotClass).toMatch(/^bg-status-/);
      expect(d.pillClass).toContain('ring-');
    }
  });
});
