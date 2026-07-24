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
 *  polygon (the polygon is the un-rounded box, used for geometry tests).
 *
 *  The corner radius `R` is fixed at 12px, sized for real node boxes
 *  (~210x80+). Painted naively at a small box — e.g. the 34x20 palette-chip
 *  preview — `R` no longer fits: the straight run between two corner curves
 *  (from `t+R` to `b-R` on the vertical edges, `l+R` to `r-R` on the
 *  horizontal ones) would need to run backwards once the box is shorter than
 *  `2*R + 2*I`. Clamping to what the box can actually hold keeps the curve
 *  monotonic at any size and is a no-op at real node sizes (210x80 clamps to
 *  12, unchanged). */
function roundedRectPath(w: number, h: number): string {
  const l = I;
  const t = I;
  const r = w - I;
  const b = h - I;
  const rad = Math.min(R, (w - 2 * I) / 2, (h - 2 * I) / 2);
  return (
    `M ${l + rad} ${t} L ${r - rad} ${t} Q ${r} ${t} ${r} ${t + rad} ` +
    `L ${r} ${b - rad} Q ${r} ${b} ${r - rad} ${b} ` +
    `L ${l + rad} ${b} Q ${l} ${b} ${l} ${b - rad} ` +
    `L ${l} ${t + rad} Q ${l} ${t} ${l + rad} ${t} Z`
  );
}

/** Clamp a horizontal inset constant (`SHEAR`/`TAPER`/`POINT`/`RAIL`) to what
 *  a small box can actually hold, the same way `roundedRectPath`'s `rad` is
 *  clamped above. Each of those constants cuts in from a box edge on BOTH
 *  sides (e.g. a hexagon's top edge runs from `x=POINT` to `x=w-POINT`); once
 *  the box is narrower than roughly `2*CONST + 2*I`, the two insets overlap
 *  and the vertex order the shape depends on reverses — a hexagon/trapezoid's
 *  top edge runs backwards, producing the self-intersecting bowtie this
 *  clamp exists to prevent. Capping at HALF the inner span
 *  (`(dim - 2*I) * 0.5`) is the largest value under which both insets can
 *  coexist without crossing (it is exactly the fraction `rad` claims of each
 *  axis above), and it leaves a guaranteed non-zero `2*I` (4px) span at the
 *  moment of clamping — `SHEAR`/`RAIL` only eat ONE side per axis, so their
 *  true bound is looser than this, but the same factor is used for every
 *  horizontal inset here for one auditable rule rather than four bespoke
 *  ones, and it is still comfortably safe for both.
 *
 *  No-op at every real node box: the largest raw constant clamped here is
 *  `TAPER` (26), and the narrowest real box (`NODE_W`/`ACTION_W`/`GATE_W` =
 *  210-214, `workflowLayout.ts`) puts `(dim - 2*I) * 0.5` at ~103-105 — far
 *  above any raw constant, so `Math.min` always keeps the unclamped value. It
 *  only engages at the 34x20 palette-preview box, where `(34 - 4) * 0.5 = 15`
 *  is below `POINT` (22) and `TAPER` (26). */
function clampInset(value: number, dim: number): number {
  return Math.min(value, (dim - 2 * I) * 0.5);
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
      const shear = clampInset(SHEAR, w);
      const points: [number, number][] = [
        [shear, I],
        [w - I, I],
        [w - shear, h - I],
        [I, h - I],
      ];
      return {
        ...base,
        points,
        path: toPath(points),
        extra: [],
        safe: { x: shear + 8, y: 11, w: w - 2 * shear - 16, h: h - 22 },
      };
    }

    case 'trapezoid': {
      const taper = clampInset(TAPER, w);
      const points: [number, number][] = [
        [taper, I],
        [w - taper, I],
        [w - I, h - I],
        [I, h - I],
      ];
      return {
        ...base,
        points,
        path: toPath(points),
        extra: [],
        safe: { x: taper + 7, y: 13, w: w - 2 * taper - 14, h: h - 26 },
      };
    }

    case 'hexagon': {
      const point = clampInset(POINT, w);
      const points: [number, number][] = [
        [point, I],
        [w - point, I],
        [w - I, h / 2],
        [w - point, h - I],
        [point, h - I],
        [I, h / 2],
      ];
      return {
        ...base,
        points,
        path: toPath(points),
        extra: [],
        safe: { x: point + 7, y: 11, w: w - 2 * point - 14, h: h - 22 },
      };
    }

    case 'subroutine': {
      const rail = clampInset(RAIL, w);
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
        extra: [`M ${rail} ${I} L ${rail} ${h - I}`, `M ${w - rail} ${I} L ${w - rail} ${h - I}`],
        safe: { x: rail + 8, y: 11, w: w - 2 * rail - 16, h: h - 22 },
      };
    }

    case 'stacked': {
      // body sits down-left; the layers peek out up-right. LAYER offsets one
      // side of BOTH axes (not two, unlike SHEAR/TAPER/POINT/RAIL above), so
      // it is clamped against each axis independently and the tighter of the
      // two wins — at the 34x20 palette box, height (20) is the binding
      // constraint, not width (34).
      const layer = Math.min(clampInset(LAYER, w), clampInset(LAYER, h));
      const points: [number, number][] = [
        [I, layer + I],
        [w - layer - I, layer + I],
        [w - layer - I, h - I],
        [I, h - I],
      ];
      return {
        ...base,
        points,
        path: toPath(points),
        extra: [
          `M ${layer} ${I + 3} L ${w - I - 3} ${I + 3} L ${w - I - 3} ${h - layer}`,
          `M ${layer - 3} ${I + 6} L ${w - I - 6} ${I + 6} L ${w - I - 6} ${h - layer - 3}`,
        ],
        safe: { x: 13, y: layer + 10, w: w - layer - 24, h: h - layer - 21 },
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
