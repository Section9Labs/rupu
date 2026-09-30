// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect } from 'vitest';
import { render, screen, cleanup } from '@testing-library/react';
import { FindingEvidence } from './FindingEvidence';
import type { FindingRecord } from '../../lib/api';

afterEach(cleanup);

function finding(rationale: string): FindingRecord {
  return {
    id: 'f1',
    severity: 'high',
    summary: 's',
    evidence: { rationale, references: [] },
  } as unknown as FindingRecord;
}

describe('FindingEvidence', () => {
  it('renders inline code in the rationale as <code>, not literal backticks', () => {
    render(<FindingEvidence finding={finding('the `parseToken` call trusts input')} />);
    const code = screen.getByText('parseToken');
    expect(code.tagName).toBe('CODE');
    expect(screen.queryByText(/`parseToken`/)).toBeNull();
  });

  it('renders blank-line-separated paragraphs as separate <p> elements', () => {
    const { container } = render(
      <FindingEvidence finding={finding('First paragraph.\n\nSecond paragraph.')} />,
    );
    const paras = Array.from(container.querySelectorAll('p')).map((p) => p.textContent);
    expect(paras).toContain('First paragraph.');
    expect(paras).toContain('Second paragraph.');
  });

  it('renders a fenced block as a <pre>', () => {
    const { container } = render(
      <FindingEvidence finding={finding('Build:\n\n```sh\ncargo build --release\n```')} />,
    );
    expect(container.querySelector('pre')).not.toBeNull();
  });

  it('keeps the rationale compact and dim via wrapper descendant variants', () => {
    const { container } = render(
      <FindingEvidence finding={finding('First paragraph.\n\n- item one\n- item two')} />,
    );
    const p = container.querySelector('p');
    expect(p).not.toBeNull();
    // Markdown renders inside its own `.prose-rupu` div; the styling wrapper is
    // that div's parent.
    const wrapper = p!.closest('.prose-rupu')?.parentElement;
    expect(wrapper).toBeTruthy();
    expect(wrapper!.className).toContain('[&_p]:text-ink-dim');
    expect(wrapper!.className).toContain('[&_p]:text-ui');
    expect(wrapper!.className).toContain('[&_p]:leading-snug');
    expect(wrapper!.className).toContain('[&_li]:text-ink-dim');
    expect(wrapper!.className).toContain('[&_li]:text-ui');
    expect(container.querySelector('li')).not.toBeNull();
  });

  it('keeps the empty-state message when there is no evidence', () => {
    render(<FindingEvidence finding={finding('')} />);
    expect(screen.getByText('No evidence recorded.')).toBeInTheDocument();
  });
});
