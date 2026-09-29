// A small filled status dot — the inline marker atom (mirrors Ghost's
// theme.Dot). Colour comes from a themed `--c-*` token via inline style so the
// same dot reads on light and dark. Static size classes only.

export type DotColor = 'brand' | 'awaiting' | 'failed' | 'done' | 'mute';

const TOKEN: Record<DotColor, string> = {
  brand: 'var(--c-brand-500)',
  awaiting: 'var(--c-status-awaiting)',
  failed: 'var(--c-status-failed)',
  done: 'var(--c-status-done)',
  mute: 'var(--c-ink-mute)',
};

export function Dot({ color = 'mute', className }: { color?: DotColor; className?: string }) {
  return (
    <span
      className={`inline-block h-2 w-2 shrink-0 rounded-full ${className ?? ''}`}
      style={{ background: `rgb(${TOKEN[color]})` }}
      aria-hidden
    />
  );
}

export default Dot;
