// Brand — the rupu identity mark: the ∞ glyph on a violet tile + the "rupu"
// wordmark (with an optional sub-label). Mirrors the rupu.sh brand so the
// Control Plane reads as the same product. Colors route through brand/ink
// tokens so the mark adapts when the dark theme lands.

interface BrandProps {
  /** Small label under the wordmark (e.g. "Control Plane"). Omit for just the mark + name. */
  sublabel?: string | null;
}

export default function Brand({ sublabel = 'Control Plane' }: BrandProps) {
  return (
    <span className="flex items-center gap-2">
      <span
        className="flex h-7 w-7 shrink-0 items-center justify-center rounded-md bg-brand-600 text-white"
        aria-hidden="true"
      >
        <span className="text-[15px] font-light leading-none">&#8734;</span>
      </span>
      <span className="leading-tight">
        <span className="block text-sm font-semibold text-ink">rupu</span>
        {sublabel && <span className="block text-[11px] text-ink-mute">{sublabel}</span>}
      </span>
    </span>
  );
}
