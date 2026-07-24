// kindVisuals — shared accent + icon maps consumed by EditableStepNode and
// NodePalette (classic + next). Value-preserving refactor: these are the
// EXACT accents both call sites used inline before this module existed.

import { describe, it, expect } from 'vitest';
import { KIND_ACCENT, KIND_ICON, KIND_SHAPE } from './kindVisuals';
import type { StepKind } from '../../lib/workflowGraph';
import type { ColorKey } from '../../lib/useThemeColors';

const KINDS: StepKind[] = ['step', 'for_each', 'parallel', 'panel', 'branch', 'approval_gate', 'action'];

const EXPECTED_ACCENT: Record<StepKind, ColorKey> = {
  step: 'status.running',
  for_each: 'brand.500',
  parallel: 'sev.critical',
  panel: 'status.awaiting',
  branch: 'status.done',
  approval_gate: 'status.paused',
  action: 'sev.info',
};

describe('kindVisuals', () => {
  it('KIND_ACCENT covers every StepKind with the exact previously-duplicated values', () => {
    for (const kind of KINDS) {
      expect(KIND_ACCENT[kind]).toBe(EXPECTED_ACCENT[kind]);
    }
    expect(Object.keys(KIND_ACCENT).sort()).toEqual([...KINDS].sort());
  });

  it('KIND_ICON covers every StepKind with a distinct lucide component', () => {
    for (const kind of KINDS) {
      expect(KIND_ICON[kind]).toBeTruthy();
    }
    expect(Object.keys(KIND_ICON).sort()).toEqual([...KINDS].sort());
    // Every kind gets a visually distinct icon (no accidental aliasing).
    expect(new Set(Object.values(KIND_ICON)).size).toBe(KINDS.length);
  });

  it('KIND_SHAPE maps every kind to its flowchart symbol', () => {
    expect(KIND_SHAPE).toEqual({
      step: 'rect',
      for_each: 'hexagon',
      parallel: 'subroutine',
      panel: 'stacked',
      branch: 'vhex',
      approval_gate: 'trapezoid',
      action: 'parallelogram',
    });
  });
});
