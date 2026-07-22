// A finding's `evidence.code_excerpt` rendered with a line-number gutter and
// syntax highlighting — the same per-line `CodeHighlight inline` + gutter
// pattern SourcePreview/CodeViewer use. Line numbers start at the finding's
// `line_range[0]` when known; language is inferred from the file path.

import CodeHighlight from '../CodeHighlight';
import { languageForPath } from '../../lib/situationRoom/lang';

export default function CodeExcerpt({
  code,
  startLine,
  filePath,
}: {
  code: string;
  startLine?: number;
  filePath?: string | null;
}) {
  const language = languageForPath(filePath);
  // Drop a single trailing newline so we don't render a blank final row.
  const lines = code.replace(/\n$/, '').split('\n');
  const gutter = startLine != null;

  return (
    <div className="sr-code" role="group" aria-label="code excerpt">
      {lines.map((line, i) => (
        <div className="sr-code-row" key={i}>
          {gutter && <span className="sr-code-gutter" aria-hidden>{startLine + i}</span>}
          <span className="sr-code-line">
            <CodeHighlight code={line.length ? line : ' '} language={language} inline />
          </span>
        </div>
      ))}
    </div>
  );
}
