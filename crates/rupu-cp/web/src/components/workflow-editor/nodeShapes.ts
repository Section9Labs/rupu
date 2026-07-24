// nodeShapes — pure silhouette geometry for the Flow Designer's `next` nodes.
//
// Each node KIND paints a flowchart symbol (see kindVisuals.KIND_SHAPE):
// step→rect, branch→diamond, action→parallelogram, approval_gate→trapezoid,
// for_each→hexagon, parallel→subroutine, panel→stacked. This module owns the
// geometry only — no React, no DOM, no colour. The component paints `path`
// (plus `extra`) into an SVG layer and positions its content inside `safe`.
//
// Two rules encoded here, both from the approved design:
//  1. `safe` is inscribed at the shape's NARROWEST row, so text can never
//     overrun the outline (truncation is bounded by the safe rect, not the
//     bounding box).
//  2. `align` is part of the shape. A silhouette whose width varies across the
//     text band (the diamond) CENTRES its content — left-aligned text there
//     starts on the slope and reads as spilling outside the outline.

/** Stroke inset, in px — keeps the 1.5px silhouette stroke off the box edge so
 *  it is never half-clipped by the SVG viewBox. */
const I = 2;
/** Corner radius of the plain `rect` silhouette (matches the old `.wfx-node`). */
const R = 12;
/** Horizontal shear of a parallelogram, per side. */
const SHEAR = 20;
/** How far a trapezoid's top edge is inset, per side. */
const TAPER = 26;
/** How far a hexagon's left/right points reach in from the box edge. */
const POINT = 22;
/** Inset of a subroutine's two vertical rails from the box edge. */
const RAIL = 11;
/** Offset of a stacked shape's layers behind its body. */
const LAYER = 9;

export type ShapeName =
  | 'rect'
  | 'diamond'
  | 'parallelogram'
  | 'trapezoid'
  | 'hexagon'
  | 'subroutine'
  | 'stacked';

/** Where content may live, in box coordinates. */
export interface SafeRect {
  x: number;
  y: number;
  w: number;
  h: number;
}

/** A handle position expressed against the box, not a hardcoded percentage of
 *  a rectangle. `offset` is a CSS length applied along `side` (`top` for
 *  left/right, `left` for bottom). */
export interface HandleAnchor {
  side: 'left' | 'right' | 'bottom';
  offset: string;
}

/** A source handle. `id` is omitted for the single default source; `branch`
 *  reports the two arms, whose ids are a MODEL CONTRACT (applyConnect reads
 *  them) even though their positions are shape-derived. */
export interface SourceAnchor {
  id?: 'then' | 'else';
  anchor: HandleAnchor;
}

export interface NodeShape {
  /** Silhouette vertices. `rect` reports its un-rounded corners. */
  points: [number, number][];
  /** SVG `d` for the filled+stroked silhouette. */
  path: string;
  /** Extra `d` strings stroked (never filled) on top — rails, stack layers. */
  extra: string[];
  safe: SafeRect;
  align: 'start' | 'center';
  target: HandleAnchor;
  sources: SourceAnchor[];
}

function toPath(points: [number, number][]): string {
  return `M ${points.map(([x, y]) => `${x} ${y}`).join(' L ')} Z`;
}

/** Rounded rectangle — the only silhouette whose painted path differs from its
 *  polygon (the polygon is the un-rounded box, used for geometry tests). */
function roundedRectPath(w: number, h: number): string {
  const l = I;
  const t = I;
  const r = w - I;
  const b = h - I;
  return (
    `M ${l + R} ${t} L ${r - R} ${t} Q ${r} ${t} ${r} ${t + R} ` +
    `L ${r} ${b - R} Q ${r} ${b} ${r - R} ${b} ` +
    `L ${l + R} ${b} Q ${l} ${b} ${l} ${b - R} ` +
    `L ${l} ${t + R} Q ${l} ${t} ${l + R} ${t} Z`
  );
}

const LEFT_TARGET: HandleAnchor = { side: 'left', offset: '50%' };
const RIGHT_SOURCE: SourceAnchor[] = [{ anchor: { side: 'right', offset: '50%' } }];

/** Geometry for one silhouette at a given box size. Pure. */
export function shapeFor(shape: ShapeName, w: number, h: number): NodeShape {
  // Shared defaults. `extra` is deliberately NOT here: `as const` would make it
  // a readonly tuple, which is not assignable to `string[]`.
  const base = { align: 'start', target: LEFT_TARGET, sources: RIGHT_SOURCE } as const;

  switch (shape) {
    case 'diamond': {
      const points: [number, number][] = [
        [w / 2, I],
        [w - I, h / 2],
        [w / 2, h - I],
        [I, h / 2],
      ];
      return {
        points,
        path: toPath(points),
        extra: [],
        // inscribed at the band's narrowest rows (y = .28h and .72h). At
        // vertical offset f*h from the top/bottom tip, a diamond's half-width
        // is exactly f*w (the boundary edge is a straight line from the tip),
        // so the safe rect's half-width (0.25w here) must stay under f*w
        // (0.28w) — an 0.03w margin, ~8px at BRANCH_W/BRANCH_H (see
        // workflowLayout.ts). Measured against BranchBodyNext's real rendered
        // content in headless Chrome: header (kindpill + id) + condition line
        // + two then/else port pills (which wrap to two rows for any
        // realistic target name) stack to ~75px tall and, once `.wfx-head`/
        // `.wfx-body` are given `align-self: stretch` under `.wfx-safe-mid`
        // (styles.css) so their own ellipsis engages, a realistic body's
        // widest row (a port pill, ~95-100px) fits with comfortable slack in
        // this 140px-wide band — the condition line still ellipsizes for long
        // conditions, by design (nodeShapes.test.ts's point-in-polygon test
        // pins these fractions independent of BRANCH_W/BRANCH_H).
        safe: { x: w * 0.25, y: h * 0.28, w: w * 0.5, h: h * 0.44 },
        align: 'center',
        target: LEFT_TARGET,
        sources: [
          { id: 'then', anchor: { side: 'right', offset: '50%' } },
          { id: 'else', anchor: { side: 'bottom', offset: '50%' } },
        ],
      };
    }

    case 'parallelogram': {
      const points: [number, number][] = [
        [SHEAR, I],
        [w - I, I],
        [w - SHEAR, h - I],
        [I, h - I],
      ];
      return {
        ...base,
        points,
        path: toPath(points),
        extra: [],
        safe: { x: SHEAR + 8, y: 11, w: w - 2 * SHEAR - 16, h: h - 22 },
      };
    }

    case 'trapezoid': {
      const points: [number, number][] = [
        [TAPER, I],
        [w - TAPER, I],
        [w - I, h - I],
        [I, h - I],
      ];
      return {
        ...base,
        points,
        path: toPath(points),
        extra: [],
        safe: { x: TAPER + 7, y: 13, w: w - 2 * TAPER - 14, h: h - 26 },
      };
    }

    case 'hexagon': {
      const points: [number, number][] = [
        [POINT, I],
        [w - POINT, I],
        [w - I, h / 2],
        [w - POINT, h - I],
        [POINT, h - I],
        [I, h / 2],
      ];
      return {
        ...base,
        points,
        path: toPath(points),
        extra: [],
        safe: { x: POINT + 7, y: 11, w: w - 2 * POINT - 14, h: h - 22 },
      };
    }

    case 'subroutine': {
      const points: [number, number][] = [
        [I, I],
        [w - I, I],
        [w - I, h - I],
        [I, h - I],
      ];
      return {
        ...base,
        points,
        path: toPath(points),
        extra: [`M ${RAIL} ${I} L ${RAIL} ${h - I}`, `M ${w - RAIL} ${I} L ${w - RAIL} ${h - I}`],
        safe: { x: RAIL + 8, y: 11, w: w - 2 * RAIL - 16, h: h - 22 },
      };
    }

    case 'stacked': {
      // body sits down-left; the layers peek out up-right.
      const points: [number, number][] = [
        [I, LAYER + I],
        [w - LAYER - I, LAYER + I],
        [w - LAYER - I, h - I],
        [I, h - I],
      ];
      return {
        ...base,
        points,
        path: toPath(points),
        extra: [
          `M ${LAYER} ${I + 3} L ${w - I - 3} ${I + 3} L ${w - I - 3} ${h - LAYER}`,
          `M ${LAYER - 3} ${I + 6} L ${w - I - 6} ${I + 6} L ${w - I - 6} ${h - LAYER - 3}`,
        ],
        safe: { x: 13, y: LAYER + 10, w: w - LAYER - 24, h: h - LAYER - 21 },
      };
    }

    case 'rect':
    default: {
      const points: [number, number][] = [
        [I, I],
        [w - I, I],
        [w - I, h - I],
        [I, h - I],
      ];
      return {
        ...base,
        points,
        path: roundedRectPath(w, h),
        extra: [],
        safe: { x: 15, y: 11, w: w - 30, h: h - 22 },
      };
    }
  }
}
