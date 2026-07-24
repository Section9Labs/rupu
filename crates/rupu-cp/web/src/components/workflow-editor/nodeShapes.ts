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
  /** Perpendicular inset, in px, in from the box edge along `side` — e.g. for
   *  `side: 'right'` this becomes a `right: <inset>px` offset instead of the
   *  default `right: 0`. Needed by any shape whose boundary at the anchored
   *  offset is not flush with the box edge (a slanted or narrowed side).
   *  Default 0 (flush with the box edge) when omitted. */
  inset?: number;
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
 *  clamp exists to prevent.
 *
 *  `fraction` of the inner span (`(dim - 2*I) * fraction`) is the cap.
 *  `0.5` (HALF the inner span) is the largest value under which both insets
 *  can coexist without crossing at all — the mathematically tightest
 *  simple-polygon bound — but simplicity alone is not the acceptance bar for
 *  every caller: at the 34x20 palette-preview box, `0.5` leaves only a 4px
 *  flat edge, which anti-aliases away at the true 24x14 CSS display size and
 *  makes a hexagon read as a diamond and a trapezoid read as a triangle
 *  (see `EDGE_CLAMP_FRACTION`). Callers pass a fraction chosen for their own
 *  recognisability requirement, not just non-self-intersection; `0.5` remains
 *  available as the loosest safe value (used by `LAYER_CLAMP_FRACTION`,
 *  which has no inversion risk at all — see its call site). */
function clampInset(value: number, dim: number, fraction: number): number {
  return Math.min(value, (dim - 2 * I) * fraction);
}

/** Fraction used for the four insets that cut in from BOTH sides of an edge
 *  (`SHEAR`/`TAPER`/`POINT`/`RAIL`) — the ones that self-intersect into a
 *  bowtie if pushed past `0.5`. `0.3` was measured empirically (see
 *  `nodeShapes.test.ts`'s recognisability test and Task 5's fix-round-2
 *  report) as the point where the palette's 34x20 preview keeps a flat edge
 *  wide enough to read as a hexagon/trapezoid rather than degenerating to a
 *  diamond/triangle at the true 24x14 CSS display size, while staying well
 *  clear of the `0.5` self-intersection boundary (9px of margin at 34x20:
 *  the `0.3` cap is 9, the `0.5` cap is 15). No-op at every real node box —
 *  see the no-op note below. */
const EDGE_CLAMP_FRACTION = 0.3;

/** Fraction used for `LAYER` (the `stacked` shape's layer offset). `LAYER`
 *  is not an edge inset — it does not cut in from both sides of the same
 *  edge, so it cannot invert the polygon's vertex order the way
 *  `EDGE_CLAMP_FRACTION`'s four constants can. Its only correctness
 *  requirement is that the offset stays smaller than the box itself
 *  (`layer < min(w, h) - 2*I`, so the inner body rect stays non-degenerate);
 *  `0.5` of the inner span satisfies that with wide margin (a value up to
 *  just under `1.0` would still be geometrically safe) and was already
 *  validated as visually correct in fix round 1 (layers clearly visible at
 *  34x20). Lowering it to `0.3` in lockstep with the edge insets — as a
 *  blind "apply the same number everywhere" move — would instead REGRESS
 *  `stacked`: at 34x20 the binding axis is height (20), where `0.3` computes
 *  to `(20-4)*0.3 = 4.8` versus `0.5`'s `8`, shrinking the layer offset by
 *  nearly half and making the stacked layers harder to see for no
 *  recognisability benefit (nothing about `stacked` was reported broken).
 *  Kept at `0.5`, unchanged from fix round 1. */
const LAYER_CLAMP_FRACTION = 0.5;

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
      const shear = clampInset(SHEAR, w, EDGE_CLAMP_FRACTION);
      const points: [number, number][] = [
        [shear, I],
        [w - I, I],
        [w - shear, h - I],
        [I, h - I],
      ];
      // Both slanted sides (p1->p2 on the right, p3->p0 on the left) cross
      // y=h/2 at their parameter midpoint (t=0.5, since y runs linearly from
      // I to h-I and h/2 is that range's own midpoint) — giving boundary x =
      // w - (shear+I)/2 on the right, (shear+I)/2 on the left. Both handles
      // sit at the box edge (x=0 or x=w) by default, so the inset needed to
      // land back on the boundary is the same (shear+I)/2 on both sides.
      const inset = (shear + I) / 2;
      return {
        ...base,
        points,
        path: toPath(points),
        extra: [],
        safe: { x: shear + 8, y: 11, w: w - 2 * shear - 16, h: h - 22 },
        target: { side: 'left', offset: '50%', inset },
        sources: [{ anchor: { side: 'right', offset: '50%', inset } }],
      };
    }

    case 'trapezoid': {
      const taper = clampInset(TAPER, w, EDGE_CLAMP_FRACTION);
      const points: [number, number][] = [
        [taper, I],
        [w - taper, I],
        [w - I, h - I],
        [I, h - I],
      ];
      // Same midpoint argument as parallelogram above, applied to the
      // trapezoid's own slanted sides: boundary x at y=h/2 is
      // w - (taper+I)/2 on the right, (taper+I)/2 on the left.
      const inset = (taper + I) / 2;
      return {
        ...base,
        points,
        path: toPath(points),
        extra: [],
        safe: { x: taper + 7, y: 13, w: w - 2 * taper - 14, h: h - 26 },
        target: { side: 'left', offset: '50%', inset },
        sources: [{ anchor: { side: 'right', offset: '50%', inset } }],
      };
    }

    case 'hexagon': {
      const point = clampInset(POINT, w, EDGE_CLAMP_FRACTION);
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
      const rail = clampInset(RAIL, w, EDGE_CLAMP_FRACTION);
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
      const layer = Math.min(
        clampInset(LAYER, w, LAYER_CLAMP_FRACTION),
        clampInset(LAYER, h, LAYER_CLAMP_FRACTION),
      );
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
        // The body rect's LEFT edge is at x=I, same as every other shape's
        // un-inset side (rect/hexagon/diamond/subroutine all sit I off their
        // box edge too) — no inset needed there. Its RIGHT edge, though, is
        // pulled in by `layer` on top of the usual `I` (to leave room for the
        // stack layers peeking out), so the default right:0 handle lands on
        // the decorative layer stroke, not the body — inset by `layer + I`.
        sources: [{ anchor: { side: 'right', offset: '50%', inset: layer + I } }],
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
