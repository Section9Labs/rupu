// cmTooltips — shared CodeMirror 6 tooltip-positioning extension.
//
// Any editor that mounts `autocompletion()` inside an `overflow`-clipped
// ancestor (the inspector rail's `overflow-y-auto`, a scrollable panel, …)
// needs this: without it, CodeMirror parents the completion popup under the
// editor's own `cm-editor` DOM node, so the popup gets clipped by whatever
// scroll container the editor sits in (matt's screenshot: dropdown cut off
// after one row). `position: 'fixed'` makes CM's popup coordinates
// viewport-based so body-parenting still tracks correctly, including on
// ancestor scroll.
//
// Originally lived only in ExpressionFieldImpl (the expression editor);
// CodeEditorImpl's markdown mode registers a real HTML-tag completion source
// too (`@codemirror/lang-markdown`), so both editors share this builder.

import type { Extension } from '@codemirror/state';
import { tooltips } from '@codemirror/view';

/** Body-parented, viewport-fixed tooltip config. Exported so it's
 *  independently testable — jsdom's CodeMirror rendering is too limited to
 *  reliably assert a completion popup escapes the DOM by mounting it, so
 *  tests assert this builder's output instead. SSR/jsdom-safe: no-ops when
 *  `document` isn't available. */
export function buildTooltipExtensions(): Extension[] {
  if (typeof document === 'undefined') return [];
  return [tooltips({ position: 'fixed', parent: document.body })];
}
