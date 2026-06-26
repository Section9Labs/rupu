/**
 * CodeHighlight — read-only syntax highlighting for definition files in the
 * Build section (workflows / agents / autoflows).
 *
 * Uses highlight.js core with only `yaml` and `markdown` registered to keep the
 * bundle lean, reusing the same GitHub light theme as the transcript markdown
 * renderer. Highlighted markup is injected via `dangerouslySetInnerHTML`; the
 * input is trusted local definition-file text and highlight.js escapes its own
 * output.
 *
 * Two modes:
 *  - `<CodeHighlight code={yaml} language="yaml" />` — highlight the whole
 *    string as one language (workflows / autoflows, which are pure YAML).
 *  - `<CodeHighlight code={raw} frontmatter />` — detect a leading YAML
 *    frontmatter block, highlight it as YAML and the rest as markdown (agent
 *    `.md` definition files).
 */

import hljs from 'highlight.js/lib/core';
import yaml from 'highlight.js/lib/languages/yaml';
import markdown from 'highlight.js/lib/languages/markdown';

// Light GitHub-style theme — matches the transcript markdown renderer; the CP
// is light-only so no dark-mode switching is needed.
import 'highlight.js/styles/github.css';

hljs.registerLanguage('yaml', yaml);
hljs.registerLanguage('markdown', markdown);

type Language = 'yaml' | 'markdown';

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
  'whitespace-pre-wrap break-words font-mono text-[12px] leading-relaxed text-ink ' +
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
}

export default function CodeHighlight({
  code,
  language = 'yaml',
  frontmatter = false,
}: CodeHighlightProps) {
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

  return (
    <pre className={PRE_CLASS}>
      <code className="hljs" dangerouslySetInnerHTML={{ __html: html }} />
    </pre>
  );
}
