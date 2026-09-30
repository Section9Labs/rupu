import { Link } from 'react-router-dom';
import CodeHighlight, { HIGHLIGHTABLE_LANGUAGES, type Language } from '../../CodeHighlight';
import { codeHref, type ClaimState, type EvidenceClaim } from '../../../lib/findingReport';

const LANG_ALIAS: Record<string, Language> = {
  ts: 'typescript',
  py: 'python',
  yml: 'yaml',
  sh: 'bash',
  js: 'javascript',
  rs: 'rust',
};

/** Map a report's free-form `lang` tag to a registered highlighter, or null. */
function resolveLang(lang?: string): Language | null {
  if (!lang) return null;
  const key = lang.trim().toLowerCase();
  const name = LANG_ALIAS[key] ?? key;
  return HIGHLIGHTABLE_LANGUAGES.has(name) ? (name as Language) : null;
}

const BADGE: Partial<Record<ClaimState, { text: string; cls: string }>> = {
  changed: { text: 'Code changed since recorded', cls: 'bg-warn-bg text-warn' },
  missing: { text: 'File no longer present', cls: 'bg-err-bg text-err' },
};

export default function EvidenceClaims({ claims, states, wsId }: { claims: EvidenceClaim[]; states: ClaimState[]; wsId?: string }) {
  return (
    <div className="space-y-2">
      {claims.map((c, i) => {
        const badge = BADGE[states[i] ?? 'unknown'];
        const lang = resolveLang(c.lang);
        const loc = c.file ? `${c.file}${c.lines ? `:${c.lines[0]}-${c.lines[1]}` : ''}` : c.binary_va;
        return (
          <div key={i} className="overflow-hidden rounded-md border border-border bg-panel">
            <div className="flex flex-wrap items-baseline gap-2 border-b border-border px-3 py-1.5">
              {loc &&
                (c.file && wsId ? (
                  <Link to={codeHref(wsId, c.file, c.lines?.[0])} className="font-mono text-note text-brand-700 hover:underline">{loc}</Link>
                ) : (
                  <span className="font-mono text-note text-ink-mute">{loc}</span>
                ))}
              <span className="min-w-0 flex-1 text-ui text-ink-dim">{c.claim}</span>
              {badge && <span className={`rounded px-1.5 py-0.5 text-note ${badge.cls}`}>{badge.text}</span>}
              {c.artifact && <span className="rounded bg-surface px-1.5 py-0.5 text-note text-ink-dim ring-1 ring-border">artifact: {c.artifact}</span>}
            </div>
            {c.excerpt &&
              (lang ? (
                <CodeHighlight code={c.excerpt} language={lang} />
              ) : (
                <pre className="overflow-x-auto px-3 py-2 text-note font-mono text-ink leading-snug whitespace-pre">{c.excerpt}</pre>
              ))}
          </div>
        );
      })}
    </div>
  );
}
