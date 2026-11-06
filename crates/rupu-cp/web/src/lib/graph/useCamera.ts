// Moves a graph's camera (xyflow's viewport): a smooth eased move (CAMERA_MS,
// in-out) that retargets an unfinished one rather than queueing behind it,
// or a jump (keyboard moves, and every move under reduced motion).

import { useCallback, useMemo } from 'react';
import { useReactFlow } from '@xyflow/react';
import { cubicBezier, EASE_IN_OUT, prefersReducedMotion } from '../chart/motion';
import { CAMERA_MS, type Viewport } from '../flow/camera';

export function useCamera(): (vp: Viewport, how?: 'smooth' | 'jump') => void {
  const rf = useReactFlow();
  const ease = useMemo(() => cubicBezier(EASE_IN_OUT), []);
  return useCallback(
    (vp: Viewport, how: 'smooth' | 'jump' = 'smooth') => {
      // A new transition interrupts the running one (d3-zoom): retarget.
      if (how === 'jump' || prefersReducedMotion()) void rf.setViewport(vp);
      else void rf.setViewport(vp, { duration: CAMERA_MS, ease, interpolate: 'linear' });
    },
    [rf, ease],
  );
}
