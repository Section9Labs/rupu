// Brand — the rupu identity mark: the ∞ glyph on a violet tile + the "rupu"
// wordmark (with an optional sub-label). Mirrors the rupu.sh brand so the
// Control Plane reads as the same product. Colors route through brand/ink
// tokens so the mark adapts when the dark theme lands.

interface BrandProps {
  /** Small label under the wordmark (e.g. "Control Plane"). Omit for just the mark + name. */
  sublabel?: string | null;
  /** `default` = the v1 sidebar lockup (unchanged). `rail` = the Shell v2
   *  48px rail header: 24px gradient tile + 13px wordmark, no sublabel —
   *  the caller places the trailing `cp` tag itself. */
  variant?: 'default' | 'rail';
}

export default function Brand({ sublabel = 'Control Plane', variant = 'default' }: BrandProps) {
  if (variant === 'rail') {
    return (
      <span className="flex items-center gap-2">
        <span
          aria-hidden="true"
          className="flex h-6 w-6 items-center justify-center rounded-md bg-gradient-to-br from-brand-500 to-brand-700 text-white shadow-[0_0_0_1px_rgb(var(--c-brand-500)/0.35)] font-mono text-[14px] font-light leading-none"
        >
          &#8734;
        </span>
        <span className="text-[13px] font-semibold text-ink">rupu</span>
      </span>
    );
  }
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
