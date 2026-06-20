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
      },
      fontFamily: {
        sans: ['-apple-system', 'BlinkMacSystemFont', 'Inter', 'system-ui', 'sans-serif'],
        mono: ['ui-monospace', 'SFMono-Regular', 'Menlo', 'monospace'],
      },
      boxShadow: {
        card: '0 1px 2px rgba(15, 23, 42, 0.04), 0 0 0 1px rgba(15, 23, 42, 0.06)',
      },
    },
  },
  plugins: [],
} satisfies Config;
