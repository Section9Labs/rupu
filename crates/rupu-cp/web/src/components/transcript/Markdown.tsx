/**
 * Markdown — lightweight prose renderer used inside transcript turns.
 *
 * Renders markdown via react-markdown with rehype-highlight for fenced
 * code blocks, styled by the shared theme-aware `codeHighlight.css` so blocks
 * track the CP's light/dark theme and sit transparently on the panel.
 *
 * Consumers: the transcript (Turn, FindingCard), the Code tab's inline
 * finding cards (InlineFindingCard) and the findings evidence panel
 * (FindingEvidence, shared by the findings tables). All of them sit behind
 * lazy-loaded routes, and the `manualChunks.markdown` group in vite.config.ts
 * ensures react-markdown & co. land in their own chunk, not the main entry.
 *
 * Images: the text is agent-written, so a Markdown image must never make the
 * operator's browser fetch a URL on render (a tracking / exfiltration channel
 * the agent controls). Every `![alt](url)` renders as a `[image: alt]` link the
 * operator can choose to open; a caller may opt in to inline same-origin
 * `/api/...` images with `allowSameOriginApiImages`. Raw HTML is never parsed
 * (no rehype-raw), so `<img>` & co. in the text stay escaped, inert text.
 */

import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import rehypeHighlight from 'rehype-highlight';
import type { Components, ExtraProps } from 'react-markdown';
import type { ComponentPropsWithoutRef } from 'react';

// Theme-aware highlight.js token palette (transparent background; light/dark
// via [data-theme] on <html>). Replaces the old global
// `highlight.js/styles/github.css`, whose `.hljs{background:#fff}` was a global
// import that painted white boxes behind code in dark mode app-wide (incl. the
// Code viewer). Covers rehype-highlight's `.hljs` fenced blocks here.
import '../codeHighlight.css';

// ---------------------------------------------------------------------------
// Typed component map — no `any`; each key is a keyof JSX.IntrinsicElements
// ---------------------------------------------------------------------------

const components: Components = {
  // Headings
  h1: ({ node: _node, children, ...props }) => (
    <h1 className="text-lg font-semibold text-ink mt-4 mb-1 leading-snug" {...props}>
      {children}
    </h1>
  ),
  h2: ({ node: _node, children, ...props }) => (
    <h2 className="text-base font-semibold text-ink mt-3 mb-1 leading-snug" {...props}>
      {children}
    </h2>
  ),
  h3: ({ node: _node, children, ...props }) => (
    <h3 className="text-sm font-semibold text-ink mt-3 mb-1 leading-snug" {...props}>
      {children}
    </h3>
  ),

  // Paragraph
  p: ({ node: _node, children, ...props }) => (
    <p className="text-sm text-ink leading-relaxed mb-2 last:mb-0" {...props}>
      {children}
    </p>
  ),

  // Unordered list
  ul: ({ node: _node, children, ...props }) => (
    <ul className="list-disc list-outside pl-5 mb-2 space-y-0.5 text-sm text-ink" {...props}>
      {children}
    </ul>
  ),

  // Ordered list
  ol: ({ node: _node, children, ...props }) => (
    <ol className="list-decimal list-outside pl-5 mb-2 space-y-0.5 text-sm text-ink" {...props}>
      {children}
    </ol>
  ),

  // List item
  li: ({ node: _node, children, ...props }) => (
    <li className="leading-relaxed" {...props}>
      {children}
    </li>
  ),

  // Inline code
  code: ({ node: _node, children, className, ...props }) => {
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
  pre: ({ node: _node, children, ...props }) => (
    <pre
      className="bg-surface border border-border rounded-md overflow-x-auto text-[0.82rem] leading-relaxed my-2 p-3 font-mono"
      {...props}
    >
      {children}
    </pre>
  ),

  // Block quote
  blockquote: ({ node: _node, children, ...props }) => (
    <blockquote
      className="border-l-2 border-brand-500 pl-3 text-sm text-ink-dim italic my-2"
      {...props}
    >
      {children}
    </blockquote>
  ),

  // Horizontal rule
  hr: ({ node: _node, ...props }) => <hr className="border-border my-3" {...props} />,

  // Hyperlinks
  a: ({ node: _node, children, href, ...props }) => (
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
  strong: ({ node: _node, children, ...props }) => (
    <strong className="font-semibold text-ink" {...props}>
      {children}
    </strong>
  ),
  em: ({ node: _node, children, ...props }) => (
    <em className="italic text-ink-dim" {...props}>
      {children}
    </em>
  ),

  // GFM table
  table: ({ node: _node, children, ...props }) => (
    <div className="overflow-x-auto my-2">
      <table className="w-full text-sm border-collapse" {...props}>
        {children}
      </table>
    </div>
  ),
  thead: ({ node: _node, children, ...props }) => (
    <thead className="border-b border-border" {...props}>
      {children}
    </thead>
  ),
  tbody: ({ node: _node, children, ...props }) => <tbody {...props}>{children}</tbody>,
  tr: ({ node: _node, children, ...props }) => (
    <tr className="border-b border-border last:border-0" {...props}>
      {children}
    </tr>
  ),
  th: ({ node: _node, children, ...props }) => (
    <th className="text-left font-semibold text-ink px-2 py-1 align-top" {...props}>
      {children}
    </th>
  ),
  td: ({ node: _node, children, ...props }) => (
    <td className="text-ink px-2 py-1 align-top" {...props}>
      {children}
    </td>
  ),
  // GFM strikethrough
  del: ({ node: _node, children, ...props }) => (
    <del className="text-ink-mute line-through" {...props}>
      {children}
    </del>
  ),
  // GFM task-list checkbox (rendered as a disabled input by remark-gfm)
  input: ({ node: _node, ...props }) => (
    <input className="mr-1 align-middle accent-brand-500" disabled {...props} />
  ),
};

// ---------------------------------------------------------------------------
// Images
// ---------------------------------------------------------------------------

type ImgProps = ComponentPropsWithoutRef<'img'> & ExtraProps;

/** True only for a URL that resolves to this origin under `/api/` (after the
 *  URL parser normalizes `..`, `\` and protocol-relative forms). */
function isSameOriginApiUrl(src: string): boolean {
  if (typeof window === 'undefined') return false;
  let url: URL;
  try {
    url = new URL(src, window.location.origin);
  } catch {
    return false;
  }
  return url.origin === window.location.origin && url.pathname.startsWith('/api/');
}

/** An image the browser does not fetch: a link to it, or inert text when the
 *  URL was stripped (react-markdown's urlTransform empties `javascript:` & co.). */
function ImageLink({ src, alt }: { src?: string; alt?: string }) {
  const label = alt ? `[image: ${alt}]` : '[image]';
  if (!src) {
    return <span className="text-ink-mute">{label}</span>;
  }
  return (
    <a
      href={src}
      title={src}
      className="text-brand-700 underline underline-offset-2 hover:text-brand-500 transition-colors"
      target="_blank"
      rel="noreferrer noopener"
    >
      {label}
    </a>
  );
}

const imgAsLink = ({ src, alt }: ImgProps) => <ImageLink src={src} alt={alt} />;

const imgSameOriginApi = ({ src, alt, title }: ImgProps) => {
  if (!src || !isSameOriginApiUrl(src)) {
    return <ImageLink src={src} alt={alt} />;
  }
  return (
    <img
      src={src}
      title={title}
      alt={alt ?? ''}
      loading="lazy"
      referrerPolicy="no-referrer"
      className="max-w-full rounded border border-border my-2"
    />
  );
};

const componentsLinkImages: Components = { ...components, img: imgAsLink };
const componentsApiImages: Components = { ...components, img: imgSameOriginApi };

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

export default function Markdown({
  text,
  allowSameOriginApiImages = false,
}: {
  text: string;
  /** Render same-origin `/api/...` images inline. Off by default: every image,
   *  remote or not, renders as a link so nothing is fetched on render. */
  allowSameOriginApiImages?: boolean;
}) {
  return (
    <div className="prose-rupu min-w-0">
      <ReactMarkdown
        remarkPlugins={[remarkGfm]}
        rehypePlugins={[rehypeHighlight]}
        components={allowSameOriginApiImages ? componentsApiImages : componentsLinkImages}
      >
        {text}
      </ReactMarkdown>
    </div>
  );
}
