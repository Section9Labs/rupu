import { describe, expect, it } from 'vitest';
import type { ThemeColors } from '../useThemeColors';
import type { RowKind } from '../messages';
import { deadPaint, messagePaint } from './messagePaint';

// A palette whose every colour is its own key, so the mapping reads directly.
const colors = { inkMute: 'inkMute', get: (k: string) => k } as unknown as ThemeColors;

describe('messagePaint', () => {
  it('each_kind_has_its_paint', () => {
    const kinds: RowKind[] = ['direct', 'mention', 'broadcast', 'post', 'directive', 'operator', 'retract'];
    expect(kinds.map((k) => messagePaint(k, colors).stroke)).toEqual([
      'chart.5',
      'chart.5',
      'chart.1',
      'chart.4',
      'chart.3',
      'status.awaiting',
      'inkMute',
    ]);
    expect(messagePaint('from-the-future', colors).stroke).toBe('chart.5');
    expect(deadPaint(colors)).toEqual({ stroke: 'status.failed', dash: '3 5' });
  });
});
