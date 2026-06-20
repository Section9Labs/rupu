// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { describe, it, expect } from 'vitest';
import { render, screen } from '@testing-library/react';
import Markdown from './Markdown';

describe('Markdown GFM', () => {
  it('renders a GFM table', () => {
    const md = '| Sev | Count |\n| --- | --- |\n| high | 3 |\n| low | 1 |';
    const { container } = render(<Markdown text={md} />);
    expect(container.querySelector('table')).toBeInTheDocument();
    expect(container.querySelectorAll('th').length).toBe(2);
    expect(screen.getByText('high')).toBeInTheDocument();
    expect(screen.getByText('3')).toBeInTheDocument();
  });

  it('renders strikethrough', () => {
    const { container } = render(<Markdown text={'~~gone~~'} />);
    expect(container.querySelector('del')).toBeInTheDocument();
  });
});
