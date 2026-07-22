// Map a file path / extension to a highlight.js language id that CodeHighlight
// actually has a grammar for (HIGHLIGHTABLE_LANGUAGES). Findings carry a
// `file_path` but no language tag, so the Situation Room infers one here.
// Unknown → 'plaintext' (registered, renders as un-highlighted text) rather
// than CodeHighlight's 'yaml' default, which would mis-color arbitrary source.

import { HIGHLIGHTABLE_LANGUAGES, type Language } from '../../components/CodeHighlight';

const EXT_TO_LANG: Record<string, Language> = {
  ts: 'typescript', mts: 'typescript', cts: 'typescript', tsx: 'typescript',
  js: 'javascript', mjs: 'javascript', cjs: 'javascript', jsx: 'javascript',
  rs: 'rust', py: 'python', pyi: 'python', go: 'go',
  json: 'json', yaml: 'yaml', yml: 'yaml', toml: 'toml',
  md: 'markdown', markdown: 'markdown',
  rb: 'ruby', java: 'java', kt: 'kotlin', kts: 'kotlin', swift: 'swift',
  c: 'c', h: 'c', cc: 'cpp', cpp: 'cpp', cxx: 'cpp', hpp: 'cpp', hh: 'cpp',
  cs: 'csharp', php: 'php',
  sh: 'bash', bash: 'bash', zsh: 'bash',
  sql: 'sql', html: 'xml', htm: 'xml', xml: 'xml', vue: 'xml', svelte: 'xml',
  css: 'css', scss: 'scss', less: 'less',
  lua: 'lua', r: 'r', scala: 'scala', pl: 'perl', pm: 'perl',
  dart: 'dart', m: 'objectivec', mm: 'objectivec',
};

// Extension-less filenames whose basename picks the language.
const NAME_TO_LANG: Record<string, Language> = {
  dockerfile: 'dockerfile',
  makefile: 'makefile',
};

/** Best-effort language for a file path. Returns 'plaintext' when unknown so
 *  the caller can still render (un-highlighted) rather than mis-highlight. */
export function languageForPath(path?: string | null): Language {
  if (!path) return 'plaintext';
  const base = path.split('/').pop() ?? path;
  const byName = NAME_TO_LANG[base.toLowerCase()];
  if (byName) return byName;
  const dot = base.lastIndexOf('.');
  if (dot < 0) return 'plaintext';
  const ext = base.slice(dot + 1).toLowerCase();
  const lang = EXT_TO_LANG[ext];
  return lang && HIGHLIGHTABLE_LANGUAGES.has(lang) ? lang : 'plaintext';
}
