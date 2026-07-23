// ExpressionFieldImpl — the CodeMirror 6 body behind the lazy `ExpressionField`.
//
// A small, single- or multi-line editor for ONE workflow expression field. It
// statically imports `@codemirror/*`; like CodeEditorImpl it is ONLY ever pulled
// in via `React.lazy`, so CodeMirror stays in an async chunk out of the main
// entry. Two behaviors set it apart from the YAML editor:
//
//   • Highlighting — a ViewPlugin paints `{{ … }}` mustache regions: the
//     delimiters, dotted paths, `| filters`, and quoted strings. Robust against
//     a partial / unclosed `{{`.
//   • Autocomplete — fires ONLY inside a `{{ … }}` region, offering the real
//     template vocabulary for this field's context (see workflowExpressions).

import { useEffect, useRef } from 'react';
import { EditorState, RangeSetBuilder, type Extension } from '@codemirror/state';
import {
  EditorView,
  keymap,
  Decoration,
  ViewPlugin,
  drawSelection,
  type DecorationSet,
  type ViewUpdate,
} from '@codemirror/view';
import { defaultKeymap, history, historyKeymap } from '@codemirror/commands';
import {
  autocompletion,
  completionKeymap,
  closeBrackets,
  closeBracketsKeymap,
  type CompletionContext,
  type CompletionResult,
  type Completion,
} from '@codemirror/autocomplete';
import { completionsFor, type ExprContext, type ExprKind } from '../../lib/workflowExpressions';
import type { ExpressionFieldProps } from './ExpressionField';
import type { Mode } from '../theme/ThemeProvider';
import { buildTooltipExtensions } from '../cmTooltips';

// Re-exported for backward compatibility — `buildTooltipExtensions` now lives
// in the shared `cmTooltips` module (CodeEditorImpl's markdown mode needs it
// too), but this module keeps exporting it so existing imports/tests
// (`ExpressionField.test.tsx`) keep working unchanged.
export { buildTooltipExtensions } from '../cmTooltips';

// ── mustache scanning (shared by highlighter + completion gate) ───────────────

/** Whether `pos` sits inside an open `{{ … }}` region (closing `}}` optional). */
function insideMustache(text: string, pos: number): boolean {
  const open = text.lastIndexOf('{{', Math.max(0, pos - 1));
  if (open === -1) return false;
  const close = text.indexOf('}}', open + 2);
  return close === -1 || close >= pos;
}

interface Tok {
  from: number;
  to: number;
  cls: string;
}

const RE_REGION = /\{\{([\s\S]*?)(\}\}|$)/g;
const RE_STRING = /'[^']*'|"[^"]*"/g;
const RE_FILTER = /\|\s*([A-Za-z_]\w*)/g;
const RE_PATH = /[A-Za-z_]\w*(?:\.[A-Za-z_]\w*)*/g;

/** Tokenize one mustache region's inner text into non-overlapping spans. */
function tokenizeInner(inner: string, base: number, out: Tok[]): void {
  const claimed: { from: number; to: number }[] = [];
  const overlaps = (from: number, to: number): boolean =>
    claimed.some((c) => from < c.to && to > c.from);
  const claim = (from: number, to: number, cls: string): void => {
    if (overlaps(from, to)) return;
    claimed.push({ from, to });
    out.push({ from: base + from, to: base + to, cls });
  };

  // Strings first (highest priority — paths inside a string must not re-tokenize).
  for (const m of inner.matchAll(RE_STRING)) {
    claim(m.index ?? 0, (m.index ?? 0) + m[0].length, 'cm-expr-string');
  }
  // Filter names (the identifier after `|`).
  for (const m of inner.matchAll(RE_FILTER)) {
    const nameStart = (m.index ?? 0) + m[0].length - m[1].length;
    claim(nameStart, nameStart + m[1].length, 'cm-expr-filter');
  }
  // Dotted paths / identifiers.
  for (const m of inner.matchAll(RE_PATH)) {
    claim(m.index ?? 0, (m.index ?? 0) + m[0].length, 'cm-expr-path');
  }
}

/** Build the decoration set for the whole doc (fields are short — no viewport
 *  windowing needed). */
function buildDecorations(view: EditorView): DecorationSet {
  const text = view.state.doc.toString();
  const toks: Tok[] = [];
  for (const m of text.matchAll(RE_REGION)) {
    const start = m.index ?? 0;
    const openEnd = start + 2;
    toks.push({ from: start, to: openEnd, cls: 'cm-expr-delim' }); // `{{`
    const inner = m[1];
    tokenizeInner(inner, openEnd, toks);
    if (m[2] === '}}') {
      const closeStart = start + m[0].length - 2;
      toks.push({ from: closeStart, to: closeStart + 2, cls: 'cm-expr-delim' }); // `}}`
    }
  }
  toks.sort((a, b) => a.from - b.from || a.to - b.to);
  const builder = new RangeSetBuilder<Decoration>();
  let last = -1;
  for (const t of toks) {
    if (t.from < last || t.to <= t.from) continue; // skip overlaps / empties
    builder.add(t.from, t.to, Decoration.mark({ class: t.cls }));
    last = t.to;
  }
  return builder.finish();
}

const mustacheHighlighter = ViewPlugin.fromClass(
  class {
    decorations: DecorationSet;
    constructor(view: EditorView) {
      this.decorations = buildDecorations(view);
    }
    update(u: ViewUpdate): void {
      if (u.docChanged || u.viewportChanged) this.decorations = buildDecorations(u.view);
    }
  },
  { decorations: (v) => v.decorations },
);

// ── autocomplete ──────────────────────────────────────────────────────────────

const TYPE_BY_KIND: Record<ExprKind, string> = {
  path: 'property',
  filter: 'function',
  function: 'function',
  loop: 'variable',
  keyword: 'keyword',
};

function makeCompletionSource(getContext: () => ExprContext) {
  return (cc: CompletionContext): CompletionResult | null => {
    const text = cc.state.doc.toString();
    if (!insideMustache(text, cc.pos)) return null;

    const word = cc.matchBefore(/[\w.]*/);
    if (!word) return null;
    if (word.from === word.to && !cc.explicit) return null;

    const prefix = word.text.toLowerCase();
    const entries = completionsFor(getContext());
    const options: Completion[] = entries
      .filter((e) => prefix === '' || e.insert.toLowerCase().startsWith(prefix))
      .map((e) => ({
        label: e.insert,
        displayLabel: e.label,
        detail: e.detail,
        type: TYPE_BY_KIND[e.kind],
        apply: e.insert,
      }));
    if (options.length === 0) return null;
    return { from: word.from, options, filter: false };
  };
}

// ── theme ─────────────────────────────────────────────────────────────────────

// Mustache token colors per mode — the light hexes wash out on near-black, so
// dark mode uses lighter, higher-contrast variants. The container's `bg-panel`
// (from the ExpressionField shell) supplies the surface; the editor itself stays
// transparent with an inherited text color + a mode-tuned caret.
function makeExprTheme(dark: boolean): Extension {
  return EditorView.theme(
    {
      '&': { fontSize: '13px', backgroundColor: 'transparent', color: 'inherit' },
      '.cm-content': {
        padding: '6px 10px',
        fontFamily: 'ui-monospace, SFMono-Regular, Menlo, monospace',
        caretColor: dark ? '#e6edf3' : '#0f172a',
      },
      '.cm-cursor, .cm-dropCursor': { borderLeftColor: dark ? '#e6edf3' : '#0f172a' },
      '.cm-scroller': { fontFamily: 'inherit', lineHeight: '1.5' },
      '.cm-selectionBackground, &.cm-focused .cm-selectionBackground, ::selection': {
        backgroundColor: dark ? 'rgba(96, 165, 250, 0.25)' : 'rgba(59, 130, 246, 0.18)',
      },
      '&.cm-focused': { outline: 'none' },
      '.cm-expr-delim': { color: dark ? '#c084fc' : '#9333ea', fontWeight: '600' },
      '.cm-expr-path': { color: dark ? '#60a5fa' : '#2563eb' },
      '.cm-expr-filter': { color: dark ? '#2dd4bf' : '#0d9488' },
      '.cm-expr-string': { color: dark ? '#fbbf24' : '#b45309' },
    },
    { dark },
  );
}

function singleLineGuard(): Extension {
  // Drop newline insertions so a single-line field stays one line.
  return EditorState.transactionFilter.of((tr) => {
    if (!tr.docChanged) return tr;
    let hasNewline = false;
    tr.changes.iterChanges((_a, _b, _c, _d, inserted) => {
      if (inserted.toString().includes('\n')) hasNewline = true;
    });
    if (!hasNewline) return tr;
    return [];
  });
}

// ── component ─────────────────────────────────────────────────────────────────

/** Resolve the effective mode: explicit prop wins, else read the live
 *  `data-theme` attribute at mount (the editor mounts imperatively). */
function resolveMode(theme: Mode | undefined): Mode {
  if (theme) return theme;
  try {
    return document.documentElement.dataset.theme === 'dark' ? 'dark' : 'light';
  } catch {
    return 'light';
  }
}

export default function ExpressionFieldImpl({
  value,
  onChange,
  context,
  multiline = false,
  ariaLabel,
  theme,
}: ExpressionFieldProps) {
  const mode = resolveMode(theme);
  const containerRef = useRef<HTMLDivElement>(null);
  const viewRef = useRef<EditorView | null>(null);
  const onChangeRef = useRef(onChange);
  onChangeRef.current = onChange;
  // Read the latest context inside the (long-lived) completion source.
  const contextRef = useRef(context);
  contextRef.current = context;

  useEffect(() => {
    const parent = containerRef.current;
    if (!parent) return;

    const updateListener = EditorView.updateListener.of((u) => {
      if (u.docChanged) onChangeRef.current(u.state.doc.toString());
    });

    const extensions: Extension[] = [
      history(),
      drawSelection(),
      closeBrackets(),
      mustacheHighlighter,
      autocompletion({ override: [makeCompletionSource(() => contextRef.current)] }),
      ...buildTooltipExtensions(),
      keymap.of([...closeBracketsKeymap, ...defaultKeymap, ...historyKeymap, ...completionKeymap]),
      makeExprTheme(mode === 'dark'),
      updateListener,
    ];
    if (multiline) extensions.push(EditorView.lineWrapping);
    else extensions.push(singleLineGuard());

    const state = EditorState.create({ doc: value, extensions });
    const view = new EditorView({ state, parent });
    if (ariaLabel) view.contentDOM.setAttribute('aria-label', ariaLabel);
    viewRef.current = view;

    return () => {
      view.destroy();
      viewRef.current = null;
    };
    // `value` intentionally omitted — external sync is handled below. `mode` IS
    // included so the field recreates with the matching token colors on toggle.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [multiline, ariaLabel, mode]);

  // Sync external value changes (e.g. a kind switch / YAML reconcile).
  useEffect(() => {
    const view = viewRef.current;
    if (!view) return;
    const current = view.state.doc.toString();
    if (value !== current) {
      view.dispatch({ changes: { from: 0, to: current.length, insert: value } });
    }
  }, [value]);

  return <div ref={containerRef} className="w-full" />;
}
