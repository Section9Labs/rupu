import type { Config } from 'tailwindcss';

export default {
  content: ['./index.html', './src/**/*.{ts,tsx}'],
  // Dark theme is opt-in via `data-theme="dark"` on <html> (set by ThemeProvider).
  darkMode: ['selector', '[data-theme="dark"]'],
  theme: {
    extend: {
      // Colors resolve to CSS-variable RGB channels (defined in src/styles.css
      // under :root / [data-theme="dark"]). The `<alpha-value>` placeholder keeps
      // Tailwind's opacity modifiers working (e.g. `bg-status-running/10`).
      colors: {
        bg: 'rgb(var(--c-bg) / <alpha-value>)',
        panel: 'rgb(var(--c-panel) / <alpha-value>)',
        surface: 'rgb(var(--c-surface) / <alpha-value>)',
        'surface-hover': 'rgb(var(--c-surface-hover) / <alpha-value>)',
        border: 'rgb(var(--c-border) / <alpha-value>)',
        ink: {
          DEFAULT: 'rgb(var(--c-ink) / <alpha-value>)',
          dim: 'rgb(var(--c-ink-dim) / <alpha-value>)',
          mute: 'rgb(var(--c-ink-mute) / <alpha-value>)',
        },
        brand: {
          50:  'rgb(var(--c-brand-50) / <alpha-value>)',
          100: 'rgb(var(--c-brand-100) / <alpha-value>)',
          500: 'rgb(var(--c-brand-500) / <alpha-value>)',
          600: 'rgb(var(--c-brand-600) / <alpha-value>)',
          700: 'rgb(var(--c-brand-700) / <alpha-value>)',
        },
        // Industry-standard severity scale: CRITICAL purple → HIGH red →
        // MEDIUM orange → LOW yellow → INFO slate. (themed per :root/dark)
        sev: {
          critical: 'rgb(var(--c-sev-critical) / <alpha-value>)',
          high:     'rgb(var(--c-sev-high) / <alpha-value>)',
          medium:   'rgb(var(--c-sev-medium) / <alpha-value>)',
          low:      'rgb(var(--c-sev-low) / <alpha-value>)',
          info:     'rgb(var(--c-sev-info) / <alpha-value>)',
          // Soft per-severity backgrounds (themed) — used by lib/severity.ts so
          // high/medium/low tints stay distinct without collapsing the ramp.
          'critical-bg': 'rgb(var(--c-sev-critical-bg) / <alpha-value>)',
          'high-bg':     'rgb(var(--c-sev-high-bg) / <alpha-value>)',
          'medium-bg':   'rgb(var(--c-sev-medium-bg) / <alpha-value>)',
          'low-bg':      'rgb(var(--c-sev-low-bg) / <alpha-value>)',
          'info-bg':     'rgb(var(--c-sev-info-bg) / <alpha-value>)',
        },
        // Unified run/step status palette — the SINGLE source of truth shared by
        // pills, the timeline, the run-graph, and session dots. Channels live in
        // `src/styles.css`; the JS descriptor map in `src/lib/status.ts` mirrors
        // them for inline (canvas/chart) use.
        status: {
          running:   'rgb(var(--c-status-running) / <alpha-value>)',
          done:      'rgb(var(--c-status-done) / <alpha-value>)',
          completed: 'rgb(var(--c-status-completed) / <alpha-value>)',
          failed:    'rgb(var(--c-status-failed) / <alpha-value>)',
          awaiting:  'rgb(var(--c-status-awaiting) / <alpha-value>)',
          pending:   'rgb(var(--c-status-pending) / <alpha-value>)',
          skipped:   'rgb(var(--c-status-skipped) / <alpha-value>)',
          cancelled: 'rgb(var(--c-status-cancelled) / <alpha-value>)',
          rejected:  'rgb(var(--c-status-rejected) / <alpha-value>)',
        },
        // Generic semantic-state tokens for ad-hoc UI (error banners, success
        // ticks, warning notes, info chips). Distinct from the run/step `status`
        // palette: use these for one-off success/error/warning/info affordances.
        err:      'rgb(var(--c-err) / <alpha-value>)',
        'err-bg': 'rgb(var(--c-err-bg) / <alpha-value>)',
        ok:       'rgb(var(--c-ok) / <alpha-value>)',
        'ok-bg':  'rgb(var(--c-ok-bg) / <alpha-value>)',
        warn:     'rgb(var(--c-warn) / <alpha-value>)',
        'warn-bg':'rgb(var(--c-warn-bg) / <alpha-value>)',
        info:     'rgb(var(--c-info) / <alpha-value>)',
        'info-bg':'rgb(var(--c-info-bg) / <alpha-value>)',
      },
      fontFamily: {
        sans: ['-apple-system', 'BlinkMacSystemFont', 'Inter', 'system-ui', 'sans-serif'],
        mono: ['ui-monospace', 'SFMono-Regular', 'Menlo', 'monospace'],
      },
      // Semantic UI type scale — replaces the ad-hoc `text-[10/11/12/13px]`
      // literals scattered across the app. Bare strings set ONLY font-size, so
      // existing `leading-*` classes keep controlling line-height (the rename is
      // visually identical). meta=labels/captions, note=secondary, ui=default
      // body, lead=slightly emphasized.
      fontSize: {
        meta: '10px',
        note: '11px',
        ui: '12px',
        lead: '13px',
      },
      boxShadow: {
        card: '0 1px 2px rgba(15, 23, 42, 0.04), 0 0 0 1px rgba(15, 23, 42, 0.06)',
      },
    },
  },
  plugins: [],
} satisfies Config;
