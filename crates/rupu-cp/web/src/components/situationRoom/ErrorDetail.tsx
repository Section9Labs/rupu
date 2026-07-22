// Renders an event error string. When the string is (or embeds) JSON, offers a
// pretty "Parsed" view (JSON-highlighted) with a "Raw" toggle back to the
// untouched text; plain messages render as-is. See parseErrorDetail.

import { useMemo, useState } from 'react';
import CodeHighlight from '../CodeHighlight';
import { cn } from '../../lib/cn';
import { parseErrorDetail } from '../../lib/situationRoom/errorDetail';

export default function ErrorDetail({ text }: { text: string }) {
  const parsed = useMemo(() => parseErrorDetail(text), [text]);
  const [view, setView] = useState<'parsed' | 'raw'>('parsed');

  if (parsed.json === undefined) {
    return <div className="sr-note whitespace-pre-wrap break-words">{text}</div>;
  }

  return (
    <div className="mt-2.5">
      <div className="flex items-center gap-1.5 mb-1.5">
        {(['parsed', 'raw'] as const).map((v) => (
          <button
            key={v}
            type="button"
            aria-pressed={view === v}
            onClick={() => setView(v)}
            className={cn(
              'text-meta uppercase tracking-wide px-2 py-0.5 rounded-full border transition-colors',
              view === v ? 'bg-ink/90 text-bg border-transparent' : 'border-border text-ink-mute hover:text-ink',
            )}
          >
            {v}
          </button>
        ))}
      </div>
      {view === 'parsed' ? (
        <>
          {parsed.prefix && <div className="text-note text-ink-dim mb-1.5">{parsed.prefix}</div>}
          <div className="sr-code sr-code-block">
            <CodeHighlight code={parsed.pretty!} language="json" inline />
          </div>
        </>
      ) : (
        <div className="sr-code sr-code-block whitespace-pre-wrap break-words">{text}</div>
      )}
    </div>
  );
}
