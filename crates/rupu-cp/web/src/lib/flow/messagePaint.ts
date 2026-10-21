// One colour per message kind, for the flow's arcs, the time axis' ticks and
// the list's tags. The only kind -> colour mapping; undelivered is dashed red.

import type { RowKind } from '../messages';
import type { ThemeColors } from '../useThemeColors';

export interface Paint {
  stroke: string;
  dash?: string;
}

export function messagePaint(kind: RowKind, colors: ThemeColors): Paint {
  switch (kind) {
    case 'direct':
    case 'mention':
      return { stroke: colors.get('chart.5') };
    case 'broadcast':
      return { stroke: colors.get('chart.1') };
    case 'post':
      return { stroke: colors.get('chart.4') };
    case 'directive':
      return { stroke: colors.get('chart.3') };
    case 'operator':
      return { stroke: colors.get('status.awaiting') };
    case 'retract':
      return { stroke: colors.inkMute };
  }
}

/** The dead end of an undelivered message. */
export function deadPaint(colors: ThemeColors): Paint {
  return { stroke: colors.get('status.failed'), dash: '3 5' };
}
