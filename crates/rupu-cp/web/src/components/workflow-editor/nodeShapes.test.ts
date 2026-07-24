import { describe, it, expect } from 'vitest';
import { shapeFor, type ShapeName } from './nodeShapes';

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

const ALL: ShapeName[] = [
  'rect',
  'diamond',
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

  it('a diamond has its four vertices at the box mid-points', () => {
    const s = shapeFor('diamond', 200, 124);
    expect(s.points).toEqual([
      [100, 2],
      [198, 62],
      [100, 122],
      [2, 62],
    ]);
  });

  it('a diamond centres its text — every other shape aligns to the start', () => {
    expect(shapeFor('diamond', 200, 124).align).toBe('center');
    for (const name of ALL.filter((n) => n !== 'diamond')) {
      expect(shapeFor(name, 220, 130).align, name).toBe('start');
    }
  });

  it('a diamond anchors then on the right vertex and else on the bottom vertex', () => {
    const s = shapeFor('diamond', 200, 124);
    expect(s.target).toEqual({ side: 'left', offset: '50%' });
    expect(s.sources).toEqual([
      { id: 'then', anchor: { side: 'right', offset: '50%' } },
      { id: 'else', anchor: { side: 'bottom', offset: '50%' } },
    ]);
    // both anchor points are real vertices of the polygon, not mid-slope
    expect(s.points).toContainEqual([198, 62]); // right vertex  == then
    expect(s.points).toContainEqual([100, 122]); // bottom vertex == else
  });

  it('every non-diamond shape has one unlabelled source on the right edge', () => {
    for (const name of ALL.filter((n) => n !== 'diamond')) {
      const s = shapeFor(name, 220, 130);
      expect(s.target, name).toEqual({ side: 'left', offset: '50%' });
      expect(s.sources, name).toEqual([{ anchor: { side: 'right', offset: '50%' } }]);
    }
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
});
