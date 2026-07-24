import { describe, it, expect } from 'vitest';
import { shapeFor, type HandleAnchor, type ShapeName } from './nodeShapes';

/** Ray-casting point-in-polygon. Points exactly ON an edge may report either
 *  way, so callers test corners that should be strictly inside. */
function inside(pt: [number, number], poly: [number, number][]): boolean {
  const [x, y] = pt;
  let hit = false;
  for (let i = 0, j = poly.length - 1; i < poly.length; j = i++) {
    const [xi, yi] = poly[i];
    const [xj, yj] = poly[j];
    if (yi > y !== yj > y && x < ((xj - xi) * (y - yi)) / (yj - yi) + xi) hit = !hit;
  }
  return hit;
}

/** True if point `q` lies on segment `p`-`r`, GIVEN `p`, `q`, `r` are already
 *  known collinear (only called from `segmentsIntersect`'s collinear
 *  branches). Used for the on-edge / touching-endpoint cases the cross-product
 *  test alone can't classify. */
function onSegment(p: [number, number], q: [number, number], r: [number, number]): boolean {
  return (
    Math.min(p[0], r[0]) - 1e-9 <= q[0] &&
    q[0] <= Math.max(p[0], r[0]) + 1e-9 &&
    Math.min(p[1], r[1]) - 1e-9 <= q[1] &&
    q[1] <= Math.max(p[1], r[1]) + 1e-9
  );
}

/** Orientation of the turn `p`->`q`->`r`: 0 collinear, 1 clockwise, 2
 *  counter-clockwise (screen coords, y-down — the sign convention doesn't
 *  matter, only that it's consistent). */
function orient(p: [number, number], q: [number, number], r: [number, number]): number {
  const v = (q[1] - p[1]) * (r[0] - q[0]) - (q[0] - p[0]) * (r[1] - q[1]);
  if (Math.abs(v) < 1e-9) return 0;
  return v > 0 ? 1 : 2;
}

/** Standard O(1) segment-intersection test (orientation + collinear-overlap
 *  fallback), used below to certify a silhouette's `points` polygon is
 *  SIMPLE — the general shape of check the palette-preview bowtie defect
 *  needed and spot-checks don't generalise to. */
function segmentsIntersect(
  p1: [number, number],
  q1: [number, number],
  p2: [number, number],
  q2: [number, number],
): boolean {
  const o1 = orient(p1, q1, p2);
  const o2 = orient(p1, q1, q2);
  const o3 = orient(p2, q2, p1);
  const o4 = orient(p2, q2, q1);
  if (o1 !== o2 && o3 !== o4) return true;
  if (o1 === 0 && onSegment(p1, p2, q1)) return true;
  if (o2 === 0 && onSegment(p1, q2, q1)) return true;
  if (o3 === 0 && onSegment(p2, p1, q2)) return true;
  if (o4 === 0 && onSegment(p2, q1, q2)) return true;
  return false;
}

/** A polygon is simple iff no two of its NON-ADJACENT edges intersect.
 *  Adjacent edges (consecutive, sharing a vertex) are always excluded — that
 *  shared vertex is an intended touch, not a self-intersection. This is
 *  exactly the check that catches a `POINT`/`TAPER`-style inversion: an
 *  inverted hexagon/trapezoid's top edge crosses the edges leading into it
 *  from the sides, which are non-adjacent to it. */
function isSimplePolygon(points: [number, number][]): boolean {
  const n = points.length;
  for (let i = 0; i < n; i++) {
    const a1 = points[i];
    const a2 = points[(i + 1) % n];
    for (let j = i + 1; j < n; j++) {
      const isAdjacent = j === (i + 1) % n || (j + 1) % n === i;
      if (isAdjacent) continue;
      const b1 = points[j];
      const b2 = points[(j + 1) % n];
      if (segmentsIntersect(a1, a2, b1, b2)) return false;
    }
  }
  return true;
}

const ALL: ShapeName[] = [
  'rect',
  'vhex',
  'parallelogram',
  'trapezoid',
  'hexagon',
  'subroutine',
  'stacked',
];

describe('shapeFor', () => {
  it.each(ALL)('%s: every corner of the safe rect lies inside the silhouette', (name) => {
    const s = shapeFor(name, 220, 130);
    const { x, y, w, h } = s.safe;
    const corners: [number, number][] = [
      [x, y],
      [x + w, y],
      [x, y + h],
      [x + w, y + h],
    ];
    for (const c of corners) {
      expect(inside(c, s.points), `${name} corner ${c.join(',')} escaped the shape`).toBe(true);
    }
  });

  it.each(ALL)('%s: the path is closed and starts with a move', (name) => {
    const s = shapeFor(name, 220, 130);
    expect(s.path.startsWith('M ')).toBe(true);
    expect(s.path.trimEnd().endsWith('Z')).toBe(true);
  });

  it('a vhex has its six vertices at the top/bottom points and the flat left/right sides', () => {
    const s = shapeFor('vhex', 200, 124);
    expect(s.points).toEqual([
      [2, 22],
      [100, 2],
      [198, 22],
      [198, 102],
      [100, 122],
      [2, 102],
    ]);
  });

  it('a vhex centres its text — every other shape aligns to the start', () => {
    expect(shapeFor('vhex', 200, 124).align).toBe('center');
    for (const name of ALL.filter((n) => n !== 'vhex')) {
      expect(shapeFor(name, 220, 130).align, name).toBe('start');
    }
  });

  it('a vhex anchors then on the right flat edge and else on the bottom point', () => {
    const s = shapeFor('vhex', 200, 124);
    expect(s.target).toEqual({ side: 'left', offset: '50%' });
    expect(s.sources).toEqual([
      { id: 'then', anchor: { side: 'right', offset: '50%' } },
      { id: 'else', anchor: { side: 'bottom', offset: '50%' } },
    ]);
    // else lands on the bottom tip — a real vertex of the polygon.
    expect(s.points).toContainEqual([100, 122]); // bottom point == else
    // then lands mid-way down the right flat edge (a vertical run between
    // the shape's two right vertices), which the flat edge covers entirely.
    expect(s.points).toContainEqual([198, 22]);
    expect(s.points).toContainEqual([198, 102]);
  });

  it('every non-vhex shape has one unlabelled source on the right edge', () => {
    // `inset` (F3) varies per shape — parallelogram/trapezoid/stacked pull
    // their anchors in from the box edge to stay on their slanted/narrowed
    // boundary (see the dedicated outline test below); side/offset stay
    // uniform across every shape regardless.
    for (const name of ALL.filter((n) => n !== 'vhex')) {
      const s = shapeFor(name, 220, 130);
      expect(s.target, name).toMatchObject({ side: 'left', offset: '50%' });
      expect(s.sources, name).toHaveLength(1);
      expect(s.sources[0].anchor, name).toMatchObject({ side: 'right', offset: '50%' });
    }
  });

  it('parallelogram/trapezoid/stacked pull their anchors in from the box edge; the rest stay flush', () => {
    const flush = ALL.filter((n) => !['vhex', 'parallelogram', 'trapezoid', 'stacked'].includes(n));
    for (const name of flush) {
      const s = shapeFor(name, 220, 130);
      expect(s.target.inset ?? 0, name).toBe(0);
      expect(s.sources[0].anchor.inset ?? 0, name).toBe(0);
    }
    // parallelogram/trapezoid inset both target AND source, symmetrically.
    for (const name of ['parallelogram', 'trapezoid'] as const) {
      const s = shapeFor(name, 220, 130);
      expect(s.target.inset, name).toBeGreaterThan(0);
      expect(s.sources[0].anchor.inset, name).toBe(s.target.inset);
    }
    // stacked only insets its source (right side lands on the stack layer
    // otherwise); its target (left) is flush like every other shape.
    const stacked = shapeFor('stacked', 220, 130);
    expect(stacked.target.inset ?? 0).toBe(0);
    expect(stacked.sources[0].anchor.inset).toBeGreaterThan(0);
  });

  it('a subroutine adds its two vertical bars as extra strokes', () => {
    const s = shapeFor('subroutine', 220, 130);
    expect(s.extra).toHaveLength(2);
    expect(s.extra[0]).toContain('M 11 2');
    expect(s.extra[1]).toContain('M 209 2');
  });

  it('a stacked shape adds two offset layer strokes behind its body', () => {
    expect(shapeFor('stacked', 220, 130).extra).toHaveLength(2);
  });

  it('shapes that steal horizontal room inset their safe rect past the slope', () => {
    // parallelogram shears by 20px per side; the safe rect must clear both.
    const p = shapeFor('parallelogram', 214, 80);
    expect(p.safe.x).toBeGreaterThanOrEqual(20);
    expect(p.safe.x + p.safe.w).toBeLessThanOrEqual(214 - 20);
  });

  it('a rect clamps its corner radius to the box at a small palette-preview size, so the straight edges do not double back', () => {
    // At a real node size (210x80) the box comfortably holds the fixed
    // R=12 radius: the Q control points still run monotonically. At a small
    // preview box (34x20, the palette chip's viewBox) an unclamped R=12
    // would make the top/bottom straight run (from t+R to b-R) reverse
    // direction, since b-R < t+R once h < 2R + 2*inset. Assert both.
    const real = shapeFor('rect', 210, 80).path;
    // I=2 (stroke inset), so t+R=14 and b-R=68 at h=80 — monotonic descent.
    expect(real).toContain('Q 208 2 208 14');
    expect(real).toContain('L 208 66');

    const small = shapeFor('rect', 34, 20).path;
    // Clamped radius at 34x20 (I=2): min(12, (34-4)/2, (20-4)/2) = 8.
    // t+R=10, b-R=10 — the straight run degenerates to zero length rather
    // than reversing (10 -> 10, never 14 -> 6).
    expect(small).toContain('Q 32 2 32 10');
    expect(small).toContain('L 32 10');
    expect(small).not.toMatch(/L 32 6\b/);
  });

  // General invariant, not more spot-checks: EVERY shape's `points` polygon
  // must be simple (no two non-adjacent edges cross) at both a real node size
  // and the palette's tiny 34x20 preview box. This is the check that would
  // have caught the hexagon/trapezoid bowtie defect (POINT/TAPER inverting
  // the top-edge vertex order below half the box width) — and, being general,
  // it catches the whole class rather than only those two known instances.
  // Deliberately checks ONLY `points`/the rendered polygon, never `safe`: at
  // 34x20 several shapes' safe rects go zero/negative-sized (e.g. rect's
  // h = 20 - 22 = -2), which is harmless because ShapePreview (NodePalette)
  // never reads `safe` — only `path`/`extra`. That is not this test's concern.
  it.each(ALL)('%s: the silhouette polygon is simple at a real node size (220x130)', (name) => {
    const s = shapeFor(name, 220, 130);
    expect(isSimplePolygon(s.points), `${name} self-intersects at 220x130`).toBe(true);
  });

  it.each(ALL)('%s: the silhouette polygon is simple at the palette preview size (34x20)', (name) => {
    const s = shapeFor(name, 34, 20);
    expect(isSimplePolygon(s.points), `${name} self-intersects at 34x20`).toBe(true);
  });

  // Simplicity (above) is necessary but not sufficient: a polygon can be
  // simple and STILL be unrecognisable if the clamp that keeps it simple is
  // too tight. `hexagon` and `trapezoid` are both built from a flat top edge
  // inset from each side (`POINT`/`TAPER`); at the palette's 34x20 box, a
  // 15px inset (the mathematically tightest simplicity bound, k=0.5) leaves
  // only a 4px flat edge, which AA blur erases at the true 24x14 CSS chip
  // size — the hexagon then reads as a plain diamond and the trapezoid as a
  // plain triangle, degenerate shapes that no longer read as their symbol.
  // Assert the flat top edge is at least a third of the box width, derived
  // straight from `points` (not a hardcoded pixel count), so this stays
  // correct if the preview box size ever changes. This must fail at k=0.5
  // (4px < 34/3 ≈ 11.3px) and pass once the clamp is loosened — see Task 5
  // fix-round-2 report for the before/after evidence.
  it.each([
    ['hexagon', 0, 1] as const,
    ['trapezoid', 0, 1] as const,
  ])('%s: the flat top edge stays at least a third of the box width at 34x20, so it stays recognisable', (name, aIdx, bIdx) => {
    const s = shapeFor(name as ShapeName, 34, 20);
    const flatWidth = Math.abs(s.points[bIdx][0] - s.points[aIdx][0]);
    expect(flatWidth, `${name} flat top edge is only ${flatWidth}px wide at 34x20`).toBeGreaterThanOrEqual(34 / 3);
  });

  // ── F3: handle anchors must resolve ON the silhouette's outline ──────────
  // Reproduces exactly what EditableStepNode.tsx's `anchorProps` does to turn
  // a HandleAnchor into a point: `side` picks the fixed coordinate (0/w for
  // left/right, h for bottom) *minus* `inset` (default 0) back toward the
  // box interior, and `offset` (always a `%` string here) picks the position
  // along the perpendicular axis.
  function resolveAnchor(anchor: HandleAnchor, w: number, h: number): [number, number] {
    const pct = parseFloat(anchor.offset) / 100;
    const inset = anchor.inset ?? 0;
    switch (anchor.side) {
      case 'left':
        return [inset, h * pct];
      case 'right':
        return [w - inset, h * pct];
      case 'bottom':
        return [w * pct, h - inset];
    }
  }

  /** Shortest distance from point `p` to the segment `a`-`b`. */
  function distToSegment(p: [number, number], a: [number, number], b: [number, number]): number {
    const [px, py] = p;
    const [ax, ay] = a;
    const [bx, by] = b;
    const dx = bx - ax;
    const dy = by - ay;
    const lenSq = dx * dx + dy * dy;
    const t = lenSq === 0 ? 0 : Math.max(0, Math.min(1, ((px - ax) * dx + (py - ay) * dy) / lenSq));
    const cx = ax + t * dx;
    const cy = ay + t * dy;
    return Math.hypot(px - cx, py - cy);
  }

  /** Shortest distance from `p` to ANY edge of the (closed) polygon `poly`. */
  function distToPolygon(p: [number, number], poly: [number, number][]): number {
    let min = Infinity;
    for (let i = 0; i < poly.length; i++) {
      const a = poly[i];
      const b = poly[(i + 1) % poly.length];
      min = Math.min(min, distToSegment(p, a, b));
    }
    return min;
  }

  // Every "currently correct" shape's anchor sits exactly `I` (the module's
  // 2px stroke inset) off the box edge, because its boundary at the anchored
  // offset IS the box edge minus that stroke inset (see e.g. rect's/hexagon's
  // points). `EPSILON` tolerates that fixed, by-design 2px gap while staying
  // far below the 11-14px gaps this test exists to catch (parallelogram
  // ~11px, trapezoid ~14px, stacked's right edge ~11px at real node sizes).
  const EPSILON = 3;

  // vhex's then/else placement (right flat edge / bottom point) is approved
  // design and, unlike the diamond it replaces, resolves onto the outline the
  // same way every other shape's anchors do — no exclusion needed here.
  it.each(ALL)(
    '%s: target + source anchors resolve onto the silhouette outline',
    (name) => {
      const s = shapeFor(name, 220, 130);
      const anchors = [s.target, ...s.sources.map((src) => src.anchor)];
      for (const a of anchors) {
        const pt = resolveAnchor(a, 220, 130);
        const dist = distToPolygon(pt, s.points);
        expect(
          dist,
          `${name} anchor ${JSON.stringify(a)} is ${dist.toFixed(1)}px off the outline`,
        ).toBeLessThanOrEqual(EPSILON);
      }
    },
  );
});
