import type { ReactNode } from 'react';

export default function Section({ id, title, hint, children }: { id: string; title: string; hint?: string; children: ReactNode }) {
  return (
    <section id={id} className="scroll-mt-4 space-y-2">
      <h2 className="flex items-baseline gap-2 text-meta font-semibold uppercase tracking-wide text-ink-mute">
        {title}
        {hint && <span className="font-mono normal-case tracking-normal text-note font-normal">{hint}</span>}
      </h2>
      {children}
    </section>
  );
}
