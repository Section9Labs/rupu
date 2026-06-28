// Shared CodeMirror HighlightStyle that mirrors highlight.js's `github.css` —
// the theme the read-only display view (CodeHighlight.tsx) already uses. The
// editor (CodeEditorImpl) previously rendered with CodeMirror's neutral
// `defaultHighlightStyle`, so the SAME definition looked different when viewed
// vs. edited. Pointing the editor at this style keeps the two identical.
//
// Colors are taken verbatim from `highlight.js/styles/github.css`:
//   #d73a49 keyword/type · #6f42c1 title/def · #005cc5 number/attr/meta/heading
//   #032f62 string/link   · #6a737d comment   · #22863a name/tag/quote
//   #735c0f list bullet   · #24292e base text · #b31d28 invalid

import { HighlightStyle } from '@codemirror/language';
import { tags as t } from '@lezer/highlight';

const RED = '#d73a49'; // keywords / types
const PURPLE = '#6f42c1'; // titles / definition & function names
const BLUE = '#005cc5'; // numbers, literals, yaml keys, attrs, meta, headings
const DARKBLUE = '#032f62'; // strings / links
const GRAY = '#6a737d'; // comments
const GREEN = '#22863a'; // tag/selector names, quotes
const OLIVE = '#735c0f'; // list bullets
const INK = '#24292e'; // base text / punctuation / emphasis
const INVALID = '#b31d28';

export const githubHighlightStyle = HighlightStyle.define([
  {
    tag: [
      t.keyword,
      t.controlKeyword,
      t.operatorKeyword,
      t.definitionKeyword,
      t.moduleKeyword,
      t.typeName,
    ],
    color: RED,
  },
  {
    tag: [t.function(t.variableName), t.function(t.propertyName), t.definition(t.name), t.macroName],
    color: PURPLE,
  },
  {
    tag: [t.number, t.integer, t.float, t.bool, t.atom, t.literal, t.null, t.unit, t.self],
    color: BLUE,
  },
  {
    tag: [t.propertyName, t.attributeName, t.meta, t.annotation, t.operator, t.labelName],
    color: BLUE,
  },
  {
    tag: [t.heading, t.heading1, t.heading2, t.heading3, t.heading4, t.heading5, t.heading6],
    color: BLUE,
    fontWeight: '600',
  },
  { tag: [t.string, t.special(t.string), t.regexp, t.character, t.attributeValue], color: DARKBLUE },
  { tag: [t.link, t.url], color: DARKBLUE, textDecoration: 'underline' },
  { tag: [t.comment, t.lineComment, t.blockComment, t.docComment], color: GRAY, fontStyle: 'italic' },
  { tag: [t.name, t.tagName, t.quote], color: GREEN },
  { tag: [t.list], color: OLIVE },
  { tag: t.strong, color: INK, fontWeight: '700' },
  { tag: t.emphasis, color: INK, fontStyle: 'italic' },
  { tag: t.strikethrough, textDecoration: 'line-through' },
  {
    tag: [t.punctuation, t.separator, t.bracket, t.contentSeparator, t.processingInstruction],
    color: INK,
  },
  { tag: t.invalid, color: INVALID },
]);

// Dark counterpart — a github-dark-ish palette so the editor stays legible on the
// near-black panel in dark mode. Same tag→role mapping as the light style above;
// only the hexes change (lifted from `highlight.js/styles/github-dark.css`):
//   #ff7b72 keyword/type · #d2a8ff title/def · #79c0ff number/attr/meta/heading
//   #a5d6ff string/link  · #8b949e comment   · #7ee787 name/tag/quote
//   #ffa657 list bullet  · #e6edf3 base text  · #ffa198 invalid
const D_RED = '#ff7b72';
const D_PURPLE = '#d2a8ff';
const D_BLUE = '#79c0ff';
const D_LIGHTBLUE = '#a5d6ff';
const D_GRAY = '#8b949e';
const D_GREEN = '#7ee787';
const D_ORANGE = '#ffa657';
const D_INK = '#e6edf3';
const D_INVALID = '#ffa198';

export const githubDarkHighlightStyle = HighlightStyle.define([
  {
    tag: [
      t.keyword,
      t.controlKeyword,
      t.operatorKeyword,
      t.definitionKeyword,
      t.moduleKeyword,
      t.typeName,
    ],
    color: D_RED,
  },
  {
    tag: [t.function(t.variableName), t.function(t.propertyName), t.definition(t.name), t.macroName],
    color: D_PURPLE,
  },
  {
    tag: [t.number, t.integer, t.float, t.bool, t.atom, t.literal, t.null, t.unit, t.self],
    color: D_BLUE,
  },
  {
    tag: [t.propertyName, t.attributeName, t.meta, t.annotation, t.operator, t.labelName],
    color: D_BLUE,
  },
  {
    tag: [t.heading, t.heading1, t.heading2, t.heading3, t.heading4, t.heading5, t.heading6],
    color: D_BLUE,
    fontWeight: '600',
  },
  { tag: [t.string, t.special(t.string), t.regexp, t.character, t.attributeValue], color: D_LIGHTBLUE },
  { tag: [t.link, t.url], color: D_LIGHTBLUE, textDecoration: 'underline' },
  { tag: [t.comment, t.lineComment, t.blockComment, t.docComment], color: D_GRAY, fontStyle: 'italic' },
  { tag: [t.name, t.tagName, t.quote], color: D_GREEN },
  { tag: [t.list], color: D_ORANGE },
  { tag: t.strong, color: D_INK, fontWeight: '700' },
  { tag: t.emphasis, color: D_INK, fontStyle: 'italic' },
  { tag: t.strikethrough, textDecoration: 'line-through' },
  {
    tag: [t.punctuation, t.separator, t.bracket, t.contentSeparator, t.processingInstruction],
    color: D_INK,
  },
  { tag: t.invalid, color: D_INVALID },
]);

/** Pick the highlight style for the current theme mode. */
export function highlightStyleFor(mode: 'light' | 'dark') {
  return mode === 'dark' ? githubDarkHighlightStyle : githubHighlightStyle;
}
