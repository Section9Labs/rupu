import type { Config } from 'tailwindcss';

export default {
  content: ['./index.html', './src/**/*.{ts,tsx}'],
  theme: {
    extend: {
      colors: {
        bg: '#fafafa',
        panel: '#ffffff',
        border: '#e5e7eb',
        ink: {
          DEFAULT: '#0f172a',
          dim: '#64748b',
          mute: '#94a3b8',
        },
        brand: {
          50:  '#f5f3ff',
          100: '#ede9fe',
          500: '#7c3aed',
          600: '#6d28d9',
          700: '#5b21b6',
        },
        // Industry-standard severity scale: CRITICAL purple → HIGH red →
        // MEDIUM orange → LOW yellow → INFO slate.
        sev: {
          critical: '#9333ea',  // purple-600
          high:     '#dc2626',  // red-600
          medium:   '#ea580c',  // orange-600
          low:      '#ca8a04',  // yellow-600
          info:     '#64748b',  // slate-500
        },
        // Unified run/step status palette — the SINGLE source of truth shared by
        // pills, the timeline, the run-graph, and session dots. Mirror these
        // values in `src/lib/status.ts` (the descriptor map) and the literal
        // hexes in `src/styles.css` (CSS can't import TS).
        status: {
          running:   '#3b82f6',  // blue-500
          done:      '#22c55e',  // green-500 (alias: completed)
          completed: '#22c55e',  // green-500
          failed:    '#ef4444',  // red-500
          awaiting:  '#f59e0b',  // amber-500
          pending:   '#94a3b8',  // slate-400
          skipped:   '#cbd5e1',  // slate-300
          cancelled: '#64748b',  // slate-500
          rejected:  '#ef4444',  // red-500
        },
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
