/**
 * CodeHighlight — read-only syntax highlighting for definition files in the
 * Build section (workflows / agents / autoflows), the CP Settings Raw tab's
 * TOML preview, and the transcript drill-down's source-line preview
 * (`SourcePreview`).
 *
 * Uses highlight.js core with a fixed set of registered grammars to keep the
 * bundle lean, styled by a local two-theme stylesheet (`codeHighlight.css`)
 * keyed off `[data-hl-theme]` so it tracks the CP's light/dark theme, mirroring
 * the GitHub light/dark hexes the transcript markdown renderer and the
 * CodeMirror editor (`codeHighlightTheme.ts`) already use. `ini`'s grammar
 * ships a built-in `toml` alias — TOML's
 * `key = value` / `[section]` syntax highlights correctly under it without
 * pulling in a dedicated TOML grammar/dependency. Highlighted markup is
 * injected via `dangerouslySetInnerHTML`; the input is trusted local
 * definition/config text and highlight.js escapes its own output.
 *
 * Three modes:
 *  - `<CodeHighlight code={yaml} language="yaml" />` — highlight the whole
 *    string as one language (workflows / autoflows, which are pure YAML; the
 *    Settings Raw tab passes `language="toml"`).
 *  - `<CodeHighlight code={raw} frontmatter />` — detect a leading YAML
 *    frontmatter block, highlight it as YAML and the rest as markdown (agent
 *    `.md` definition files).
 *  - `<CodeHighlight code={line} language="rust" inline />` — highlight a
 *    single fragment (e.g. one source line) as an inline `<code>` span with
 *    no block wrapper, for `SourcePreview`'s line-numbered gutter layout.
 */

import { useContext } from 'react';
import hljs from 'highlight.js/lib/core';
import yaml from 'highlight.js/lib/languages/yaml';
import markdown from 'highlight.js/lib/languages/markdown';
import ini from 'highlight.js/lib/languages/ini';
import rust from 'highlight.js/lib/languages/rust';
import python from 'highlight.js/lib/languages/python';
import typescript from 'highlight.js/lib/languages/typescript';
import javascript from 'highlight.js/lib/languages/javascript';
import go from 'highlight.js/lib/languages/go';
import json from 'highlight.js/lib/languages/json';
import c from 'highlight.js/lib/languages/c';
import cpp from 'highlight.js/lib/languages/cpp';
import csharp from 'highlight.js/lib/languages/csharp';
import java from 'highlight.js/lib/languages/java';
import kotlin from 'highlight.js/lib/languages/kotlin';
import swift from 'highlight.js/lib/languages/swift';
import ruby from 'highlight.js/lib/languages/ruby';
import php from 'highlight.js/lib/languages/php';
import bash from 'highlight.js/lib/languages/bash';
import shell from 'highlight.js/lib/languages/shell';
import sql from 'highlight.js/lib/languages/sql';
import xml from 'highlight.js/lib/languages/xml';
import css from 'highlight.js/lib/languages/css';
import scss from 'highlight.js/lib/languages/scss';
import less from 'highlight.js/lib/languages/less';
import dockerfile from 'highlight.js/lib/languages/dockerfile';
import makefile from 'highlight.js/lib/languages/makefile';
import lua from 'highlight.js/lib/languages/lua';
import r from 'highlight.js/lib/languages/r';
import scala from 'highlight.js/lib/languages/scala';
import perl from 'highlight.js/lib/languages/perl';
import dart from 'highlight.js/lib/languages/dart';
import objectivec from 'highlight.js/lib/languages/objectivec';
import plaintext from 'highlight.js/lib/languages/plaintext';

import { ThemeContext } from './theme/ThemeProvider';

// Two-theme hljs token palette, selected at runtime via `[data-hl-theme]` —
// replaces the old light-only `highlight.js/styles/github.css` import so this
// component can switch with the CP's light/dark theme (see codeHighlight.css).
import './codeHighlight.css';

hljs.registerLanguage('yaml', yaml);
hljs.registerLanguage('markdown', markdown);
// Registers the `toml` alias too (see module doc above).
hljs.registerLanguage('ini', ini);
hljs.registerLanguage('rust', rust);
hljs.registerLanguage('python', python);
hljs.registerLanguage('typescript', typescript);
hljs.registerLanguage('javascript', javascript);
hljs.registerLanguage('go', go);
hljs.registerLanguage('json', json);
hljs.registerLanguage('c', c);
hljs.registerLanguage('cpp', cpp);
hljs.registerLanguage('csharp', csharp);
hljs.registerLanguage('java', java);
hljs.registerLanguage('kotlin', kotlin);
hljs.registerLanguage('swift', swift);
hljs.registerLanguage('ruby', ruby);
hljs.registerLanguage('php', php);
hljs.registerLanguage('bash', bash);
hljs.registerLanguage('shell', shell);
hljs.registerLanguage('sql', sql);
hljs.registerLanguage('xml', xml);
hljs.registerLanguage('css', css);
hljs.registerLanguage('scss', scss);
hljs.registerLanguage('less', less);
hljs.registerLanguage('dockerfile', dockerfile);
hljs.registerLanguage('makefile', makefile);
hljs.registerLanguage('lua', lua);
hljs.registerLanguage('r', r);
hljs.registerLanguage('scala', scala);
hljs.registerLanguage('perl', perl);
hljs.registerLanguage('dart', dart);
hljs.registerLanguage('objectivec', objectivec);
hljs.registerLanguage('plaintext', plaintext);

export type Language =
  | 'yaml'
  | 'markdown'
  | 'toml'
  | 'rust'
  | 'python'
  | 'typescript'
  | 'javascript'
  | 'go'
  | 'json'
  | 'c'
  | 'cpp'
  | 'csharp'
  | 'java'
  | 'kotlin'
  | 'swift'
  | 'ruby'
  | 'php'
  | 'bash'
  | 'shell'
  | 'sql'
  | 'xml'
  | 'css'
  | 'scss'
  | 'less'
  | 'dockerfile'
  | 'makefile'
  | 'lua'
  | 'r'
  | 'scala'
  | 'perl'
  | 'dart'
  | 'objectivec'
  | 'plaintext';

/** Languages registered above — used by `SourcePreview` to guard against
 *  highlighting with a language hljs doesn't know about (falls back to
 *  plain, unhighlighted text). Kept narrow (its original set) so
 *  `SourcePreview`'s existing behaviour/tests are untouched; `CodeViewer`
 *  (the Code tab) drives off the broader `HIGHLIGHTABLE_LANGUAGES` below. */
export const SOURCE_PREVIEW_LANGUAGES: ReadonlySet<string> = new Set([
  'rust',
  'python',
  'typescript',
  'javascript',
  'go',
  'json',
]);

/** Every hljs grammar registered above — used by `CodeViewer` (the Code tab)
 *  to decide whether a backend-detected `language` tag can be highlighted.
 *  Any language not in this set falls back to plain, unhighlighted text. */
export const HIGHLIGHTABLE_LANGUAGES: ReadonlySet<string> = new Set([
  'yaml',
  'markdown',
  'toml',
  'rust',
  'python',
  'typescript',
  'javascript',
  'go',
  'json',
  'c',
  'cpp',
  'csharp',
  'java',
  'kotlin',
  'swift',
  'ruby',
  'php',
  'bash',
  'shell',
  'sql',
  'xml',
  'css',
  'scss',
  'less',
  'dockerfile',
  'makefile',
  'lua',
  'r',
  'scala',
  'perl',
  'dart',
  'objectivec',
  'plaintext',
]);

// Matches a leading `---` … `---` frontmatter fence. The body capture keeps its
// original line endings; an unterminated fence simply doesn't match.
const FRONTMATTER_RE = /^---\r?\n([\s\S]*?)\r?\n---\r?\n?([\s\S]*)$/;

/**
 * Split a definition file into its leading YAML frontmatter block (without the
 * `---` fences) and the remaining body. Returns `frontmatter: null` when there
 * is no well-formed leading frontmatter fence.
 */
export function splitFrontmatter(raw: string): { frontmatter: string | null; body: string } {
  const m = raw.match(FRONTMATTER_RE);
  if (!m) return { frontmatter: null, body: raw };
  return { frontmatter: m[1], body: m[2] };
}

function highlight(code: string, language: Language): string {
  return hljs.highlight(code, { language, ignoreIllegals: true }).value;
}

const PRE_CLASS =
  'whitespace-pre-wrap break-words font-mono text-ui leading-relaxed text-ink ' +
  'bg-panel border border-border rounded-xl shadow-card p-4 overflow-x-auto';

export interface CodeHighlightProps {
  code: string;
  /** Language for whole-string highlighting. Ignored when `frontmatter`. */
  language?: Language;
  /**
   * Detect a leading YAML frontmatter block and highlight it as YAML, the rest
   * as markdown. Used for agent `.md` definitions.
   */
  frontmatter?: boolean;
  /**
   * Render a bare `<code>` fragment (no `<pre>` block wrapper/padding/border)
   * for embedding inline in another layout, e.g. one row per source line in
   * `SourcePreview`. Ignored when `frontmatter`.
   */
  inline?: boolean;
}

export default function CodeHighlight({
  code,
  language = 'yaml',
  frontmatter = false,
  inline = false,
}: CodeHighlightProps) {
  // Provider-optional: read the theme from context when a <ThemeProvider> is
  // present (the app path), falling back to the live `data-theme` attribute
  // instead of throwing — mirrors `useThemeColors`/`CodeEditor`'s pattern so
  // isolated tests and detached previews that render CodeHighlight without a
  // provider (SourcePreview, AgentDetail, RawEditor, ...) keep working.
  const themeCtx = useContext(ThemeContext);
  const mode =
    themeCtx?.mode ??
    (typeof document !== 'undefined' && document.documentElement.dataset.theme === 'dark'
      ? 'dark'
      : 'light');
  let html: string;
  if (frontmatter) {
    const { frontmatter: fm, body } = splitFrontmatter(code);
    if (fm !== null) {
      // Re-add the `---` fences so the rendered definition matches the file.
      const fmHtml = highlight(`---\n${fm}\n---`, 'yaml');
      const bodyHtml = body ? highlight(body, 'markdown') : '';
      html = bodyHtml ? `${fmHtml}\n${bodyHtml}` : fmHtml;
    } else {
      html = highlight(body, 'markdown');
    }
  } else {
    html = highlight(code, language);
  }

  if (inline) {
    return (
      <code
        className="hljs whitespace-pre font-mono"
        data-hl-theme={mode}
        dangerouslySetInnerHTML={{ __html: html }}
      />
    );
  }

  // Covers both the whole-string mode and the `frontmatter` mode (agent `.md`
  // definitions) — both render through this block wrapper.
  return (
    <pre className={PRE_CLASS}>
      <code className="hljs" data-hl-theme={mode} dangerouslySetInnerHTML={{ __html: html }} />
    </pre>
  );
}
