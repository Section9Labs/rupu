// ReportFields — a CLI JSON report rendered as a key/value strip: each
// scalar field one row (the key with underscores as spaces), each nested
// object its own titled group below. Empty values (null, "", "-") are left
// out, as the CLI's text output leaves them out.

import type { ReactNode } from 'react';

type Json = string | number | boolean | null | Json[] | { [k: string]: Json };

const label = (key: string) => key.replace(/_/g, ' ');

function isEmpty(v: unknown): boolean {
  return v === null || v === undefined || v === '' || v === '-' || (Array.isArray(v) && v.length === 0);
}

function scalar(v: unknown): ReactNode {
  if (Array.isArray(v)) return v.map(String).join(', ');
  if (typeof v === 'boolean') return v ? 'yes' : 'no';
  return String(v);
}

export function ReportFields({ data, title }: { data: Record<string, unknown>; title?: string }) {
  const entries = Object.entries(data).filter(([, v]) => !isEmpty(v));
  const flat = entries.filter(([, v]) => typeof v !== 'object' || Array.isArray(v));
  const nested = entries.filter(([, v]) => typeof v === 'object' && v !== null && !Array.isArray(v)) as [
    string,
    Record<string, Json>,
  ][];
  if (flat.length === 0 && nested.length === 0) return null;
  return (
    <section className="space-y-2">
      {title && <h3 className="text-note font-semibold uppercase tracking-wide text-ink-mute">{title}</h3>}
      {flat.length > 0 && (
        <dl className="divide-y divide-border rounded-lg border border-border">
          {flat.map(([k, v]) => (
            <div key={k} className="flex gap-3 px-3 py-1.5 text-note">
              <dt className="w-32 shrink-0 text-ink-mute">{label(k)}</dt>
              <dd className="min-w-0 flex-1 break-words font-mono text-ink">{scalar(v)}</dd>
            </div>
          ))}
        </dl>
      )}
      {nested.map(([k, v]) => (
        <ReportFields key={k} data={v} title={label(k)} />
      ))}
    </section>
  );
}

export default ReportFields;
