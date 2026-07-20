import { describe, it, expect } from 'vitest';
import { resolveDragRange } from './resolveDragRange';

describe('resolveDragRange', () => {
  it('orders a forward drag (A -> B) as {startDay: A, endDay: B}', () => {
    expect(resolveDragRange('2026-07-10', '2026-07-14')).toEqual({
      startDay: '2026-07-10',
      endDay: '2026-07-14',
    });
  });

  it('orders a reversed drag (B -> A) the same as the forward drag', () => {
    expect(resolveDragRange('2026-07-14', '2026-07-10')).toEqual({
      startDay: '2026-07-10',
      endDay: '2026-07-14',
    });
  });

  it('treats start === end (a click, no drag) as a no-op', () => {
    expect(resolveDragRange('2026-07-10', '2026-07-10')).toBeNull();
  });

  it('treats a missing end (pointer released outside the plot) as a no-op', () => {
    expect(resolveDragRange('2026-07-10', undefined)).toBeNull();
  });

  it('treats a missing start as a no-op', () => {
    expect(resolveDragRange(undefined, '2026-07-10')).toBeNull();
  });

  it('treats both endpoints missing as a no-op', () => {
    expect(resolveDragRange(undefined, undefined)).toBeNull();
  });
});
