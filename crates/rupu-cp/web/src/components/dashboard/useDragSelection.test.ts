// @vitest-environment jsdom
import { describe, it, expect, vi } from 'vitest';
import { act, renderHook } from '@testing-library/react';
import { useDragSelection } from './useDragSelection';

describe('useDragSelection', () => {
  it('has no band and inert handlers before any drag', () => {
    const { result } = renderHook(() => useDragSelection(() => {}));
    expect(result.current.band).toBeNull();
  });

  it('a forward drag A -> B calls onSelectRange("A", "B") on mouseup', () => {
    const onSelectRange = vi.fn();
    const { result } = renderHook(() => useDragSelection(onSelectRange));

    act(() => result.current.onMouseDown({ activeLabel: '2026-07-10' }));
    act(() => result.current.onMouseMove({ activeLabel: '2026-07-14' }));
    act(() => result.current.finalizeDrag());

    expect(onSelectRange).toHaveBeenCalledWith('2026-07-10', '2026-07-14');
  });

  it('a reversed drag B -> A orders the call as (A, B)', () => {
    const onSelectRange = vi.fn();
    const { result } = renderHook(() => useDragSelection(onSelectRange));

    act(() => result.current.onMouseDown({ activeLabel: '2026-07-14' }));
    act(() => result.current.onMouseMove({ activeLabel: '2026-07-10' }));
    act(() => result.current.finalizeDrag());

    expect(onSelectRange).toHaveBeenCalledWith('2026-07-10', '2026-07-14');
  });

  it('a click with no movement (down then up on the same bucket) does not fire onSelectRange', () => {
    const onSelectRange = vi.fn();
    const { result } = renderHook(() => useDragSelection(onSelectRange));

    act(() => result.current.onMouseDown({ activeLabel: '2026-07-10' }));
    act(() => result.current.finalizeDrag());

    expect(onSelectRange).not.toHaveBeenCalled();
  });

  it('ignores a mousedown with no activeLabel (clicked outside the plot)', () => {
    const onSelectRange = vi.fn();
    const { result } = renderHook(() => useDragSelection(onSelectRange));

    act(() => result.current.onMouseDown({ activeLabel: undefined }));
    expect(result.current.band).toBeNull();

    act(() => result.current.finalizeDrag());
    expect(onSelectRange).not.toHaveBeenCalled();
  });

  it('onMouseMove before any mousedown is a no-op', () => {
    const onSelectRange = vi.fn();
    const { result } = renderHook(() => useDragSelection(onSelectRange));

    act(() => result.current.onMouseMove({ activeLabel: '2026-07-10' }));
    expect(result.current.band).toBeNull();
  });

  it('mouseleave finalizes an in-progress drag the same as mouseup (pointer released outside the plot)', () => {
    const onSelectRange = vi.fn();
    const { result } = renderHook(() => useDragSelection(onSelectRange));

    act(() => result.current.onMouseDown({ activeLabel: '2026-07-10' }));
    act(() => result.current.onMouseMove({ activeLabel: '2026-07-12' }));
    // Simulate the pointer leaving the plot — the chart calls the same
    // `finalizeDrag` for onMouseLeave as it does for onMouseUp.
    act(() => result.current.finalizeDrag());

    expect(onSelectRange).toHaveBeenCalledWith('2026-07-10', '2026-07-12');
    expect(result.current.band).toBeNull();
  });

  it('reflects the in-progress band while dragging', () => {
    const { result } = renderHook(() => useDragSelection(() => {}));

    act(() => result.current.onMouseDown({ activeLabel: '2026-07-10' }));
    expect(result.current.band).toEqual({ start: '2026-07-10', current: '2026-07-10' });

    act(() => result.current.onMouseMove({ activeLabel: '2026-07-12' }));
    expect(result.current.band).toEqual({ start: '2026-07-10', current: '2026-07-12' });
  });

  it('clears the band after finalize', () => {
    const { result } = renderHook(() => useDragSelection(() => {}));

    act(() => result.current.onMouseDown({ activeLabel: '2026-07-10' }));
    act(() => result.current.onMouseMove({ activeLabel: '2026-07-12' }));
    act(() => result.current.finalizeDrag());

    expect(result.current.band).toBeNull();
  });

  describe('inert when onSelectRange is undefined', () => {
    it('mousedown does not start a drag (no band)', () => {
      const { result } = renderHook(() => useDragSelection(undefined));

      act(() => result.current.onMouseDown({ activeLabel: '2026-07-10' }));

      expect(result.current.band).toBeNull();
    });

    it('a full down/move/up sequence never calls anything and never sets a band', () => {
      const { result } = renderHook(() => useDragSelection(undefined));

      act(() => result.current.onMouseDown({ activeLabel: '2026-07-10' }));
      act(() => result.current.onMouseMove({ activeLabel: '2026-07-14' }));
      act(() => result.current.finalizeDrag());

      expect(result.current.band).toBeNull();
    });
  });
});
