// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect, vi } from 'vitest';
import { render, cleanup } from '@testing-library/react';
import CodeHighlight from './CodeHighlight';
import { ThemeProvider } from './theme/ThemeProvider';

// jsdom in this project has no native `matchMedia` (it throws), and
// `ThemeProvider` resolves its `system` preference from it — not from any
// dataset attribute pre-set on `document.documentElement`. Install a
// controllable mock so the resolved `mode` can be driven deterministically,
// mirroring the pattern already used in `theme/ThemeProvider.test.tsx`.
function installMatchMedia(dark: boolean) {
  vi.stubGlobal(
    'matchMedia',
    vi.fn().mockImplementation((query: string) => ({
      matches: dark,
      media: query,
      onchange: null,
      addEventListener: () => {},
      removeEventListener: () => {},
      addListener: () => {},
      removeListener: () => {},
      dispatchEvent: () => true,
    })),
  );
}

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

function renderWithTheme(ui: React.ReactNode) {
  return render(<ThemeProvider>{ui}</ThemeProvider>);
}

describe('CodeHighlight theming', () => {
  it('stamps the resolved theme mode on the rendered code element', () => {
    installMatchMedia(true);
    const { container } = renderWithTheme(
      <CodeHighlight code={'fn main() {}'} language="rust" inline />,
    );
    const code = container.querySelector('code.hljs');
    expect(code).not.toBeNull();
    expect(code!.getAttribute('data-hl-theme')).toBe('dark');
  });

  it('uses light when the document theme is light', () => {
    installMatchMedia(false);
    const { container } = renderWithTheme(
      <CodeHighlight code={'fn main() {}'} language="rust" inline />,
    );
    expect(container.querySelector('code.hljs')!.getAttribute('data-hl-theme')).toBe('light');
  });
});
