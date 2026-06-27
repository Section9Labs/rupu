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

const exprTheme = EditorView.theme({
  '&': { fontSize: '13px' },
  '.cm-content': { padding: '6px 10px', fontFamily: 'ui-monospace, SFMono-Regular, Menlo, monospace' },
  '.cm-scroller': { fontFamily: 'inherit', lineHeight: '1.5' },
  '&.cm-focused': { outline: 'none' },
  '.cm-expr-delim': { color: '#9333ea', fontWeight: '600' },
  '.cm-expr-path': { color: '#2563eb' },
  '.cm-expr-filter': { color: '#0d9488' },
  '.cm-expr-string': { color: '#b45309' },
});

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

export default function ExpressionFieldImpl({
  value,
  onChange,
  context,
  multiline = false,
  ariaLabel,
}: ExpressionFieldProps) {
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
      keymap.of([...closeBracketsKeymap, ...defaultKeymap, ...historyKeymap, ...completionKeymap]),
      exprTheme,
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
    // `value` intentionally omitted — external sync is handled below.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [multiline, ariaLabel]);

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
