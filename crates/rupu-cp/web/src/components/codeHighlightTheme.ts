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
