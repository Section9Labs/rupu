// Polylines drawn with rounded corners: the routed connectors of the graphs
// (the agentiflow map's message arcs and tree trunks). Pure; no React. A
// route is a list of points; `roundedPath` draws it with each inner corner
// rounded, `cutAt` keeps its first fraction (a dead end stops short), and
// `pointAt` finds a point along it (where a tooltip sits).

export interface Pt {
  x: number;
  y: number;
}

const f = (n: number) => Math.round(n * 10) / 10;
const dist = (a: Pt, b: Pt) => Math.hypot(b.x - a.x, b.y - a.y);

/** Drops points that repeat the one before them. */
function clean(points: Pt[]): Pt[] {
  const out: Pt[] = [];
  for (const p of points) {
    const last = out[out.length - 1];
    if (!last || dist(last, p) > 0.01) out.push(p);
  }
  return out;
}

/** The SVG path of `points`, each inner corner rounded by up to `radius`
 *  (never more than half of either leg, so short legs stay straight). */
export function roundedPath(points: Pt[], radius: number): string {
  const ps = clean(points);
  if (ps.length === 0) return '';
  let d = `M ${f(ps[0].x)} ${f(ps[0].y)}`;
  for (let i = 1; i < ps.length; i++) {
    const p = ps[i];
    const next = ps[i + 1];
    if (!next) {
      d += ` L ${f(p.x)} ${f(p.y)}`;
      break;
    }
    const prev = ps[i - 1];
    const r = Math.min(radius, dist(prev, p) / 2, dist(p, next) / 2);
    const inLen = dist(prev, p) || 1;
    const outLen = dist(p, next) || 1;
    const a = { x: p.x + ((prev.x - p.x) / inLen) * r, y: p.y + ((prev.y - p.y) / inLen) * r };
    const b = { x: p.x + ((next.x - p.x) / outLen) * r, y: p.y + ((next.y - p.y) / outLen) * r };
    d += ` L ${f(a.x)} ${f(a.y)} Q ${f(p.x)} ${f(p.y)} ${f(b.x)} ${f(b.y)}`;
  }
  return d;
}

function length(ps: Pt[]): number {
  let n = 0;
  for (let i = 1; i < ps.length; i++) n += dist(ps[i - 1], ps[i]);
  return n;
}

/** The point a fraction `t` of the way along `points`. */
export function pointAt(points: Pt[], t: number): Pt {
  const ps = clean(points);
  if (ps.length < 2) return ps[0] ?? { x: 0, y: 0 };
  let left = Math.max(0, Math.min(1, t)) * length(ps);
  for (let i = 1; i < ps.length; i++) {
    const leg = dist(ps[i - 1], ps[i]);
    if (left <= leg) {
      const k = leg === 0 ? 0 : left / leg;
      return { x: ps[i - 1].x + (ps[i].x - ps[i - 1].x) * k, y: ps[i - 1].y + (ps[i].y - ps[i - 1].y) * k };
    }
    left -= leg;
  }
  return ps[ps.length - 1];
}

/** `points` up to a fraction `t` of the way along. */
export function cutAt(points: Pt[], t: number): Pt[] {
  const ps = clean(points);
  if (ps.length < 2 || t >= 1) return ps;
  let left = Math.max(0, t) * length(ps);
  const out = [ps[0]];
  for (let i = 1; i < ps.length; i++) {
    const leg = dist(ps[i - 1], ps[i]);
    if (left <= leg) {
      const k = leg === 0 ? 0 : left / leg;
      out.push({ x: ps[i - 1].x + (ps[i].x - ps[i - 1].x) * k, y: ps[i - 1].y + (ps[i].y - ps[i - 1].y) * k });
      return out;
    }
    out.push(ps[i]);
    left -= leg;
  }
  return out;
}
