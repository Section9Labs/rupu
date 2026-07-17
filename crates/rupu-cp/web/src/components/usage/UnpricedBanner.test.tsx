// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { describe, it, expect, afterEach } from 'vitest';
import { render, screen, cleanup } from '@testing-library/react';
import { UnpricedBanner } from './UnpricedBanner';

afterEach(() => {
  cleanup();
});

describe('UnpricedBanner', () => {
  it('names the unpriced models rather than showing a bare asterisk', () => {
    render(<UnpricedBanner unpriced={{ models: ['mystery-model', 'other-model'], rows: 42 }} />);
    expect(screen.getByText(/mystery-model/)).toBeInTheDocument();
    expect(screen.getByText(/2 models unpriced/)).toBeInTheDocument();
  });

  it('renders nothing when everything is priced', () => {
    const { container } = render(<UnpricedBanner unpriced={{ models: [], rows: 0 }} />);
    expect(container).toBeEmptyDOMElement();
  });
});
