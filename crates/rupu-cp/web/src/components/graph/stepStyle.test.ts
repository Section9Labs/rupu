// @vitest-environment jsdom
// stepStyle — verifies the run-graph node's per-state visual vocabulary. The
// `paused` state (Task 8, CP Pause/Resume UI) must render with a color/glyph/
// label distinct from every other state — in particular `awaiting_approval`
// (an approval gate) and `running`, which a paused step is neither.

import { afterEach, describe, expect, it } from 'vitest';
import { readThemeColors } from '../../lib/useThemeColors';
import { stateStyle, glyphBg } from './stepStyle';

const VARS: Record<string, string> = {
  '--c-status-running': '59 130 246',
  '--c-status-done': '34 197 94',
  '--c-status-completed': '34 197 94',
  '--c-status-failed': '239 68 68',
  '--c-status-awaiting': '245 158 11',
  '--c-status-paused': '6 182 212',
  '--c-status-pending': '148 163 184',
  '--c-status-skipped': '203 213 225',
  '--c-status-cancelled': '100 116 139',
  '--c-status-rejected': '239 68 68',
};

function applyVars(vars: Record<string, string>): void {
  for (const [k, v] of Object.entries(vars)) {
    document.documentElement.style.setProperty(k, v);
  }
}

afterEach(() => {
  document.documentElement.removeAttribute('style');
});

describe('stateStyle — paused', () => {
  it('renders a color distinct from running, awaiting_approval, done, and failed', () => {
    applyVars(VARS);
    const c = readThemeColors();

    const paused = stateStyle(c, 'paused');
    const running = stateStyle(c, 'running');
    const awaiting = stateStyle(c, 'awaiting_approval');
    const done = stateStyle(c, 'done');
    const failed = stateStyle(c, 'failed');

    expect(paused.color).toBe('rgb(6 182 212)');
    expect(paused.color).not.toBe(running.color);
    expect(paused.color).not.toBe(awaiting.color);
    expect(paused.color).not.toBe(done.color);
    expect(paused.color).not.toBe(failed.color);
  });

  it('uses a distinct glyph/label from awaiting_approval', () => {
    applyVars(VARS);
    const c = readThemeColors();

    const paused = stateStyle(c, 'paused');
    const awaiting = stateStyle(c, 'awaiting_approval');

    expect(paused.label).toBe('paused');
    expect(paused.label).not.toBe(awaiting.label);
    expect(paused.glyph).not.toBe(awaiting.glyph);
  });

  it('glyphBg resolves the paused token (not the pending fallback)', () => {
    applyVars(VARS);
    const c = readThemeColors();
    expect(glyphBg(c, 'paused')).toBe('rgb(6 182 212)');
  });
});
