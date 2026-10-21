// Exit ghosts — how a card or edge deleted on the editor canvas leaves.
//
// The canvas is controlled: a delete reaches the parent through `onChange` at
// once, and the element is gone on the next render. To let it leave instead of
// vanish without delaying that change, the canvas calls `ghost(...)` just
// before `onChange`, while the element is still in the DOM: each one is cloned
// into a layer inside xyflow's viewport (so the copy pans and zooms with the
// canvas), plays the `wfx-exit` fade (styles/graph-motion.css), and is dropped.
// The copies are inert, hidden from assistive tech and carry no ids, so nothing
// can query, focus or click them while they fade.

import { useCallback, useEffect, useRef, type ReactElement } from 'react';
import { ViewportPortal } from '@xyflow/react';

/** How long a ghost lives: the `wfx-exit` animation (160ms) plus slack. */
const GHOST_MS = 240;
const SVG_NS = 'http://www.w3.org/2000/svg';
const ID_ATTRS = ['data-id', 'data-testid', 'data-nodeid', 'data-handleid', 'id'];

function scrub(el: Element) {
  for (const target of [el, ...el.querySelectorAll('*')]) {
    for (const attr of ID_ATTRS) {
      // A gradient's own `id` must stay: the cloned paths paint through it.
      if (attr === 'id' && target.closest('defs')) continue;
      target.removeAttribute(attr);
    }
  }
  el.setAttribute('aria-hidden', 'true');
  el.setAttribute('inert', '');
}

export interface ExitGhosts {
  /** Render inside `<ReactFlow>`. */
  layer: ReactElement;
  /** Clone these nodes and edges into the layer and fade them out. Call it
   *  before the change that removes them; ids not on screen are skipped. */
  ghost: (nodeIds: readonly string[], edgeIds: readonly string[]) => void;
}

export function useExitGhosts(): ExitGhosts {
  const layerRef = useRef<HTMLDivElement>(null);
  const timers = useRef(new Set<ReturnType<typeof setTimeout>>());
  // Ids fading now: xyflow reports a node's delete and its edges' deletes as
  // two changes, and each may ask for the same edge.
  const fading = useRef(new Set<string>());

  useEffect(() => {
    const pending = timers.current;
    return () => {
      for (const t of pending) clearTimeout(t);
      pending.clear();
    };
  }, []);

  const ghost = useCallback((nodeIds: readonly string[], edgeIds: readonly string[]) => {
    const layer = layerRef.current;
    const flow = layer?.closest('.react-flow');
    if (!layer || !flow) return;
    const clones: Element[] = [];
    const keys: string[] = [];
    const take = (key: string) => {
      if (fading.current.has(key)) return false;
      keys.push(key);
      return true;
    };
    const byId = (selector: string) =>
      new Map([...flow.querySelectorAll(selector)].map((el) => [el.getAttribute('data-id'), el]));
    const edgeEls = byId('.react-flow__edge');
    const nodeEls = byId('.react-flow__node');

    for (const id of new Set(edgeIds)) {
      const g = edgeEls.get(id);
      if (!g || !take(`e:${id}`)) continue;
      const svg = document.createElementNS(SVG_NS, 'svg');
      svg.setAttribute('class', 'wfx-ghost-edges');
      const copy = g.cloneNode(true) as Element;
      scrub(copy);
      copy.classList.add('wfx-ghost', 'wfx-ghost-edge');
      svg.appendChild(copy);
      clones.push(svg);
    }
    for (const id of new Set(nodeIds)) {
      const el = nodeEls.get(id);
      if (!el || !take(`n:${id}`)) continue;
      const copy = el.cloneNode(true) as Element;
      scrub(copy);
      copy.classList.add('wfx-ghost', 'wfx-ghost-node');
      clones.push(copy);
    }
    if (clones.length === 0) return;
    for (const c of clones) layer.appendChild(c);
    for (const k of keys) fading.current.add(k);
    const t = setTimeout(() => {
      timers.current.delete(t);
      for (const c of clones) c.remove();
      for (const k of keys) fading.current.delete(k);
    }, GHOST_MS);
    timers.current.add(t);
  }, []);

  const layer = (
    <ViewportPortal>
      <div ref={layerRef} className="wfx-ghosts" aria-hidden />
    </ViewportPortal>
  );
  return { layer, ghost };
}
