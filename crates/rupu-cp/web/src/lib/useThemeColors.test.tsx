// @vitest-environment jsdom
// useThemeColors — verifies the hook/reader resolves the CURRENT theme's CSS
// variables into ready `rgb(…)` strings, flipping with `data-theme`, and that the
// `get` / `alpha` helpers wrap channels correctly.

import { afterEach, describe, expect, it } from 'vitest';
import { render, screen, cleanup } from '@testing-library/react';
import ThemeProvider from '../components/theme/ThemeProvider';
import { readThemeColors, useThemeColors } from './useThemeColors';

// Minimal light / dark channel fixtures. Mirrors the shape of styles.css: a
// space-separated RGB triple per token.
const LIGHT: Record<string, string> = {
  '--c-bg': '250 250 250',
  '--c-panel': '255 255 255',
  '--c-surface': '241 245 249',
  '--c-surface-hover': '226 232 240',
  '--c-border': '229 231 235',
  '--c-ink': '15 23 42',
  '--c-ink-dim': '100 116 139',
  '--c-ink-mute': '148 163 184',
  '--c-brand-500': '124 58 237',
  '--c-brand-600': '109 40 217',
  '--c-brand-700': '91 33 182',
  '--c-status-running': '59 130 246',
  '--c-status-done': '34 197 94',
  '--c-status-completed': '34 197 94',
  '--c-status-failed': '239 68 68',
  '--c-status-awaiting': '245 158 11',
  '--c-status-pending': '148 163 184',
  '--c-status-skipped': '203 213 225',
  '--c-status-cancelled': '100 116 139',
  '--c-status-rejected': '239 68 68',
  '--c-sev-critical': '147 51 234',
  '--c-sev-high': '220 38 38',
  '--c-sev-medium': '234 88 12',
  '--c-sev-low': '202 138 4',
  '--c-sev-info': '100 116 139',
};

const DARK: Record<string, string> = {
  ...LIGHT,
  '--c-panel': '20 20 22',
  '--c-ink': '245 245 245',
  '--c-status-running': '96 165 250',
  '--c-sev-high': '248 113 113',
};

function applyVars(vars: Record<string, string>): void {
  for (const [k, v] of Object.entries(vars)) {
    document.documentElement.style.setProperty(k, v);
  }
}

afterEach(() => {
  cleanup();
  document.documentElement.removeAttribute('style');
  document.documentElement.removeAttribute('data-theme');
});

describe('readThemeColors', () => {
  it('returns light values under the light palette', () => {
    document.documentElement.dataset.theme = 'light';
    applyVars(LIGHT);
    const c = readThemeColors();
    expect(c.ink).toBe('rgb(15 23 42)');
    expect(c.panel).toBe('rgb(255 255 255)');
    expect(c.status.running).toBe('rgb(59 130 246)');
    expect(c.sev.high).toBe('rgb(220 38 38)');
  });

  it('returns dark values under the dark palette', () => {
    document.documentElement.dataset.theme = 'dark';
    applyVars(DARK);
    const c = readThemeColors();
    expect(c.ink).toBe('rgb(245 245 245)');
    expect(c.panel).toBe('rgb(20 20 22)');
    expect(c.status.running).toBe('rgb(96 165 250)');
    expect(c.sev.high).toBe('rgb(248 113 113)');
  });

  it('get()/alpha() resolve any token key', () => {
    applyVars(LIGHT);
    const c = readThemeColors();
    expect(c.get('brand.500')).toBe('rgb(124 58 237)');
    expect(c.alpha('status.running', 0.12)).toBe('rgb(59 130 246 / 0.12)');
    expect(c.brand[700]).toBe('rgb(91 33 182)');
  });
});

describe('useThemeColors (hook)', () => {
  function Probe() {
    const c = useThemeColors();
    return <span data-testid="ink">{c.ink}</span>;
  }

  it('reads the applied theme through the provider', () => {
    // ThemeProvider applies data-theme synchronously during render; default
    // preference is "system" → light in the jsdom matchMedia stub.
    applyVars(LIGHT);
    render(
      <ThemeProvider>
        <Probe />
      </ThemeProvider>,
    );
    expect(screen.getByTestId('ink').textContent).toBe('rgb(15 23 42)');
  });
});
