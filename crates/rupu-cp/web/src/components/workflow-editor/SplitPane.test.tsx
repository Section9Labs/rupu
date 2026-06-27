// @vitest-environment jsdom
// SplitPane — renders top/bottom content with an accessible resizable divider.
// jsdom has no layout, so we exercise the keyboard path (which doesn't depend on
// pointer geometry) to prove the ratio updates and the ARIA contract holds.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect } from 'vitest';
import { render, screen, cleanup, fireEvent } from '@testing-library/react';
import SplitPane from './SplitPane';

afterEach(cleanup);

describe('SplitPane', () => {
  it('renders the given top and bottom content', () => {
    render(<SplitPane top={<div>TOP CONTENT</div>} bottom={<div>BOTTOM CONTENT</div>} />);
    expect(screen.getByText('TOP CONTENT')).toBeInTheDocument();
    expect(screen.getByText('BOTTOM CONTENT')).toBeInTheDocument();
  });

  it('exposes a horizontal separator with the right ARIA contract', () => {
    render(<SplitPane top={<div />} bottom={<div />} defaultRatio={0.62} />);
    const sep = screen.getByRole('separator');
    expect(sep).toHaveAttribute('aria-orientation', 'horizontal');
    expect(sep).toHaveAttribute('aria-valuenow', '62');
    expect(sep).toHaveAttribute('tabindex', '0');
  });

  it('ArrowDown grows the top pane, ArrowUp shrinks it (aria-valuenow tracks)', () => {
    render(<SplitPane top={<div />} bottom={<div />} defaultRatio={0.5} />);
    const sep = screen.getByRole('separator');
    expect(sep).toHaveAttribute('aria-valuenow', '50');

    fireEvent.keyDown(sep, { key: 'ArrowDown' });
    expect(sep).toHaveAttribute('aria-valuenow', '53');

    fireEvent.keyDown(sep, { key: 'ArrowUp' });
    fireEvent.keyDown(sep, { key: 'ArrowUp' });
    expect(sep).toHaveAttribute('aria-valuenow', '47');
  });

  it('clamps to maxRatio so the top pane never exceeds the bound', () => {
    render(<SplitPane top={<div />} bottom={<div />} defaultRatio={0.79} maxRatio={0.8} />);
    const sep = screen.getByRole('separator');
    fireEvent.keyDown(sep, { key: 'ArrowDown' });
    fireEvent.keyDown(sep, { key: 'ArrowDown' });
    expect(sep).toHaveAttribute('aria-valuenow', '80');
  });
});
