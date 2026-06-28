/**
 * Markdown — lightweight prose renderer used inside transcript turns.
 *
 * Renders markdown via react-markdown with rehype-highlight for fenced
 * code blocks.  Import a light GitHub theme so highlighted blocks look
 * clean against the rupu panel background.
 *
 * This file lives in the `transcript/` component tree, which is already
 * reached only through the lazy-loaded RunTranscript route.  The
 * `manualChunks.markdown` group in vite.config.ts ensures that once this
 * import is reachable it lands in its own chunk, not the main entry.
 */

import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import rehypeHighlight from 'rehype-highlight';
import type { Components } from 'react-markdown';

// Light GitHub-style syntax-highlight theme — no dark-mode switching needed
// for the current rupu palette (light UI only).
import 'highlight.js/styles/github.css';

// ---------------------------------------------------------------------------
// Typed component map — no `any`; each key is a keyof JSX.IntrinsicElements
// ---------------------------------------------------------------------------

const components: Components = {
  // Headings
  h1: ({ children, ...props }) => (
    <h1 className="text-lg font-semibold text-ink mt-4 mb-1 leading-snug" {...props}>
      {children}
    </h1>
  ),
  h2: ({ children, ...props }) => (
    <h2 className="text-base font-semibold text-ink mt-3 mb-1 leading-snug" {...props}>
      {children}
    </h2>
  ),
  h3: ({ children, ...props }) => (
    <h3 className="text-sm font-semibold text-ink mt-3 mb-1 leading-snug" {...props}>
      {children}
    </h3>
  ),

  // Paragraph
  p: ({ children, ...props }) => (
    <p className="text-sm text-ink leading-relaxed mb-2 last:mb-0" {...props}>
      {children}
    </p>
  ),

  // Unordered list
  ul: ({ children, ...props }) => (
    <ul className="list-disc list-outside pl-5 mb-2 space-y-0.5 text-sm text-ink" {...props}>
      {children}
    </ul>
  ),

  // Ordered list
  ol: ({ children, ...props }) => (
    <ol className="list-decimal list-outside pl-5 mb-2 space-y-0.5 text-sm text-ink" {...props}>
      {children}
    </ol>
  ),

  // List item
  li: ({ children, ...props }) => (
    <li className="leading-relaxed" {...props}>
      {children}
    </li>
  ),

  // Inline code
  code: ({ children, className, ...props }) => {
    // rehype-highlight attaches a `language-*` class to fenced blocks; when
    // that's present the code element is inside a <pre> (block), not inline.
    // We still apply the highlight.js CSS; the rupu-specific styling below is
    // limited to the structural wrapper (handled by `pre`).
    const isBlock = typeof className === 'string' && className.startsWith('language-');
    if (isBlock) {
      return (
        <code className={className} {...props}>
          {children}
        </code>
      );
    }
    return (
      <code
        className="bg-surface rounded px-1 font-mono text-[0.9em] text-ink"
        {...props}
      >
        {children}
      </code>
    );
  },

  // Fenced code block wrapper
  pre: ({ children, ...props }) => (
    <pre
      className="bg-surface border border-border rounded-md overflow-x-auto text-[0.82rem] leading-relaxed my-2 p-3 font-mono"
      {...props}
    >
      {children}
    </pre>
  ),

  // Block quote
  blockquote: ({ children, ...props }) => (
    <blockquote
      className="border-l-2 border-brand-500 pl-3 text-sm text-ink-dim italic my-2"
      {...props}
    >
      {children}
    </blockquote>
  ),

  // Horizontal rule
  hr: (props) => <hr className="border-border my-3" {...props} />,

  // Hyperlinks
  a: ({ children, href, ...props }) => (
    <a
      href={href}
      className="text-brand-700 underline underline-offset-2 hover:text-brand-500 transition-colors"
      target="_blank"
      rel="noreferrer noopener"
      {...props}
    >
      {children}
    </a>
  ),

  // Strong / em
  strong: ({ children, ...props }) => (
    <strong className="font-semibold text-ink" {...props}>
      {children}
    </strong>
  ),
  em: ({ children, ...props }) => (
    <em className="italic text-ink-dim" {...props}>
      {children}
    </em>
  ),

  // GFM table
  table: ({ children, ...props }) => (
    <div className="overflow-x-auto my-2">
      <table className="w-full text-sm border-collapse" {...props}>
        {children}
      </table>
    </div>
  ),
  thead: ({ children, ...props }) => (
    <thead className="border-b border-border" {...props}>
      {children}
    </thead>
  ),
  tbody: ({ children, ...props }) => <tbody {...props}>{children}</tbody>,
  tr: ({ children, ...props }) => (
    <tr className="border-b border-border last:border-0" {...props}>
      {children}
    </tr>
  ),
  th: ({ children, ...props }) => (
    <th className="text-left font-semibold text-ink px-2 py-1 align-top" {...props}>
      {children}
    </th>
  ),
  td: ({ children, ...props }) => (
    <td className="text-ink px-2 py-1 align-top" {...props}>
      {children}
    </td>
  ),
  // GFM strikethrough
  del: ({ children, ...props }) => (
    <del className="text-ink-mute line-through" {...props}>
      {children}
    </del>
  ),
  // GFM task-list checkbox (rendered as a disabled input by remark-gfm)
  input: ({ ...props }) => (
    <input className="mr-1 align-middle accent-brand-500" disabled {...props} />
  ),
};

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

export default function Markdown({ text }: { text: string }) {
  return (
    <div className="prose-rupu min-w-0">
      <ReactMarkdown
        remarkPlugins={[remarkGfm]}
        rehypePlugins={[rehypeHighlight]}
        components={components}
      >
        {text}
      </ReactMarkdown>
    </div>
  );
}
