// cmTooltips — shared CodeMirror 6 tooltip-positioning + theming extensions.
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
//
// ── theming (final-review round 2 fix) ──────────────────────────────────
// A body-parented tooltip is NOT a descendant of the editor's own DOM, so it
// can't be reached by nesting selectors under `.cm-editor` in a stylesheet —
// but CM still copies `EditorView.themeClasses` onto the tooltip's DOM
// container (`TooltipViewManager`/hover-tooltip code parents the tooltip
// under `document.body` and stamps the SAME theme classes the editor's own
// wrapper carries). That means an `EditorView.theme()` extension — NOT a
// plain global stylesheet rule — is the only thing that can theme these
// tooltips: `EditorView.theme(spec)` scopes every top-level selector with a
// generated class, and that class rides along onto the tooltip container.
//
// A plain `.cm-tooltip { … }` rule in styles.css loses to CodeMirror's own
// base theme (`&light .cm-tooltip` / `&dark .cm-tooltip`, compiled to
// `.<base-light-or-dark-class> .cm-tooltip`, i.e. two classes = (0,2,0))
// because a bare `.cm-tooltip` selector is only (0,1,0). Going through
// `EditorView.theme()` instead means OUR rules get scoped the same way
// (`.<our-theme-class> .cm-tooltip` = (0,2,0) too) — an exact specificity
// tie, which CSS breaks by cascade order, and CM mounts custom `theme()`
// style modules after its base themes, so we win the tie.
//
// A couple of rules need extra care to actually tie (not lose):
//   - the autocomplete list's font/max-height: CM's own base rule is nested
//     under the compound selector `.cm-tooltip.cm-tooltip-autocomplete`
//     (two classes), so our key mirrors that same compound selector rather
//     than the single class — otherwise we'd be (0,2,1) against its (0,3,1)
//     and lose outright.
//   - the selected-row background: CM's base rule is
//     `&light/&dark .cm-tooltip-autocomplete ul li[aria-selected]`, which
//     compiles to a single scope class + `.cm-tooltip-autocomplete` (two
//     classes total) — matching our own two-class scoping (our theme class +
//     `.cm-tooltip-autocomplete`) without any extra doubling needed.
//   - `.cm-completionDetail` / `.cm-completionMatchedText` are styled by
//     CM's base theme with a BARE (unscoped-beyond-base) selector, so our
//     theme-scoped version is already strictly higher specificity and wins
//     outright, no tie-breaking needed.
//
// CSS custom properties resolve fine inside `EditorView.theme()` — the
// values are just string declarations handed to style-mod, and `--c-*` is
// set on `:root`/`[data-theme]`, an ancestor of `document.body` either way,
// so `rgb(var(--c-*))` still flips with the theme automatically.

import type { Extension } from '@codemirror/state';
import { EditorView, tooltips } from '@codemirror/view';

const MONO_STACK = 'ui-monospace, SFMono-Regular, Menlo, monospace';

const tooltipTheme = EditorView.theme({
  '.cm-tooltip': {
    background: 'rgb(var(--c-panel))',
    border: '1px solid rgb(var(--c-border))',
    borderRadius: '8px',
    color: 'rgb(var(--c-ink))',
    boxShadow: '0 8px 24px rgb(0 0 0 / .18), 0 2px 6px rgb(0 0 0 / .12)',
    overflow: 'hidden',
  },
  // Compound selector (not just `.cm-tooltip-autocomplete > ul`) to match
  // CM's own `.cm-tooltip.cm-tooltip-autocomplete { '& > ul': {...} }`
  // specificity — see file-header note.
  '.cm-tooltip.cm-tooltip-autocomplete > ul': {
    fontFamily: MONO_STACK,
    fontSize: '12px',
    maxHeight: '16rem',
    overflowY: 'auto',
  },
  '.cm-tooltip.cm-tooltip-autocomplete > ul > li': {
    padding: '4px 8px',
  },
  '.cm-tooltip-autocomplete ul li[aria-selected]': {
    background: 'rgb(var(--c-brand-500) / .16)',
    color: 'rgb(var(--c-ink))',
  },
  '.cm-tooltip-autocomplete .cm-completionLabel': {
    color: 'rgb(var(--c-ink))',
  },
  '.cm-tooltip-autocomplete .cm-completionDetail': {
    fontStyle: 'normal',
    color: 'rgb(var(--c-ink-mute))',
    marginLeft: '6px',
  },
  '.cm-tooltip-autocomplete .cm-completionMatchedText': {
    textDecoration: 'none',
    color: 'rgb(var(--c-brand-600))',
    fontWeight: '600',
  },
});

/** Body-parented, viewport-fixed, app-themed tooltip config. Exported so it's
 *  independently testable — jsdom's CodeMirror rendering is too limited to
 *  reliably assert a completion popup escapes the DOM by mounting it, so
 *  tests assert this builder's output instead. SSR/jsdom-safe: no-ops when
 *  `document` isn't available. */
export function buildTooltipExtensions(): Extension[] {
  if (typeof document === 'undefined') return [];
  return [tooltips({ position: 'fixed', parent: document.body }), tooltipTheme];
}
