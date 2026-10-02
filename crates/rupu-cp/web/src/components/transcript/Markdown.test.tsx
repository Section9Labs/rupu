// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect } from 'vitest';
import { cleanup, render, screen } from '@testing-library/react';
import Markdown from './Markdown';

afterEach(cleanup);

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

// Agent-written Markdown must never make the operator's browser fetch a URL
// on render (a tracking / exfiltration channel the agent controls).
describe('Markdown images', () => {
  const REMOTE = 'https://example.invalid/p.png';

  it('renders a remote image as a link, never an <img>', () => {
    const { container } = render(<Markdown text={`![x](${REMOTE})`} />);
    expect(container.querySelector('img')).toBeNull();
    const link = screen.getByRole('link', { name: '[image: x]' });
    expect(link).toHaveAttribute('href', REMOTE);
    expect(link).toHaveAttribute('rel', expect.stringContaining('noreferrer'));
  });

  it('labels an image with no alt text', () => {
    const { container } = render(<Markdown text={`![](${REMOTE})`} />);
    expect(container.querySelector('img')).toBeNull();
    expect(screen.getByRole('link', { name: '[image]' })).toHaveAttribute('href', REMOTE);
  });

  it('does not fetch reference-style or protocol-relative images', () => {
    const md = `![a][ref] ![b](//example.invalid/q.png)\n\n[ref]: ${REMOTE}`;
    const { container } = render(<Markdown text={md} />);
    expect(container.querySelector('img')).toBeNull();
    expect(screen.getByRole('link', { name: '[image: a]' })).toHaveAttribute('href', REMOTE);
  });

  it('does not fetch same-origin /api images unless the caller opts in', () => {
    const { container } = render(<Markdown text={'![chart](/api/findings/f1/artifacts/abc)'} />);
    expect(container.querySelector('img')).toBeNull();
    expect(screen.getByRole('link', { name: '[image: chart]' })).toBeInTheDocument();
  });

  it('renders a same-origin /api image when the caller opts in', () => {
    const { container } = render(
      <Markdown text={'![chart](/api/findings/f1/artifacts/abc)'} allowSameOriginApiImages />,
    );
    const img = container.querySelector('img');
    expect(img).not.toBeNull();
    expect(img).toHaveAttribute('src', '/api/findings/f1/artifacts/abc');
    expect(img).toHaveAttribute('alt', 'chart');
  });

  it('keeps refusing remote, non-/api and path-escaping images even when opted in', () => {
    const md = [
      `![r](${REMOTE})`,
      '![p](//example.invalid/api/p.png)',
      '![b](/\\example.invalid/api/p.png)',
      '![o](/other/p.png)',
      '![t](/api/../other/p.png)',
    ].join(' ');
    const { container } = render(<Markdown text={md} allowSameOriginApiImages />);
    expect(container.querySelector('img')).toBeNull();
    for (const name of ['r', 'p', 'b', 'o', 't']) {
      expect(screen.getByRole('link', { name: `[image: ${name}]` })).toBeInTheDocument();
    }
  });

  it('renders a javascript: image as inert text, not a link', () => {
    const { container } = render(<Markdown text={'![x](javascript:alert(1))'} />);
    expect(container.querySelector('img')).toBeNull();
    expect(container.querySelector('a')).toBeNull();
    expect(container).toHaveTextContent('[image: x]');
  });
});

describe('Markdown raw HTML', () => {
  it('keeps raw HTML inert: no elements, no fetches, no handlers', () => {
    const md = [
      '<img src="https://example.invalid/p.png" onerror="alert(1)">',
      '',
      '<script>alert(1)</script>',
      '',
      '<iframe src="https://example.invalid/f"></iframe>',
      '',
      'inline <img src="https://example.invalid/i.png"> and <a href="javascript:alert(1)">x</a>',
      '',
      '<picture><source srcset="https://example.invalid/s.png"></picture>',
    ].join('\n');
    const { container } = render(<Markdown text={md} />);
    for (const tag of ['img', 'script', 'iframe', 'picture', 'source']) {
      expect(container.querySelector(tag)).toBeNull();
    }
    expect(container.querySelector('a')).toBeNull();
    expect(container.querySelector('[onerror]')).toBeNull();
    // Without rehype-raw the markup survives only as escaped, visible text.
    expect(container).toHaveTextContent('<script>alert(1)</script>');
  });
});
