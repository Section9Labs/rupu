// @vitest-environment jsdom
import { describe, it, expect, afterEach, beforeEach, vi } from 'vitest';
import { render, screen, cleanup, act } from '@testing-library/react';
import { parseCallHash, findToolCallEl, useToolCallAnchor } from './toolCallAnchor';

const scrollSpy = vi.fn();

beforeEach(() => {
  vi.useFakeTimers();
  scrollSpy.mockReset();
  Element.prototype.scrollIntoView = scrollSpy;
});
afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

function Harness({ hash, ids }: { hash: string; ids: string[] }) {
  const st = useToolCallAnchor(hash);
  return (
    <div>
      {ids.map((id) => (
        <div key={id} data-call-id={id} data-testid={`card-${id}`} />
      ))}
      <span data-testid="status">{st.status}</span>
    </div>
  );
}

describe('parseCallHash', () => {
  it('decodes the encodeURIComponent-encoded id', () => {
    expect(parseCallHash('#call-toolu_01')).toBe('toolu_01');
    expect(parseCallHash(`#call-${encodeURIComponent('a/b:c d')}`)).toBe('a/b:c d');
  });
  it('rejects other hashes, empty ids and malformed escapes', () => {
    expect(parseCallHash('')).toBeNull();
    expect(parseCallHash('#other')).toBeNull();
    expect(parseCallHash('#call-')).toBeNull();
    expect(parseCallHash('#call-%E0%A4%A')).toBeNull();
  });
});

describe('findToolCallEl', () => {
  it('matches the raw id without CSS escaping', () => {
    render(<Harness hash="" ids={['a/b:c', 'x']} />);
    expect(findToolCallEl('a/b:c')).toBe(screen.getByTestId('card-a/b:c'));
    expect(findToolCallEl('nope')).toBeNull();
  });
});

describe('useToolCallAnchor', () => {
  it('scrolls the card whose data-call-id matches the decoded hash', () => {
    const id = 'call/1:x';
    render(<Harness hash={`#call-${encodeURIComponent(id)}`} ids={['other', id]} />);
    expect(scrollSpy).toHaveBeenCalledTimes(1);
    expect(scrollSpy.mock.instances[0]).toBe(screen.getByTestId(`card-${id}`));
    expect(screen.getByTestId('status').textContent).toBe('found');
  });

  it('keeps looking while the transcript loads, then scrolls', () => {
    const { rerender } = render(<Harness hash="#call-late" ids={[]} />);
    expect(scrollSpy).not.toHaveBeenCalled();
    expect(screen.getByTestId('status').textContent).toBe('searching');
    rerender(<Harness hash="#call-late" ids={['late']} />);
    act(() => {
      vi.advanceTimersByTime(250);
    });
    expect(scrollSpy).toHaveBeenCalledTimes(1);
    expect(screen.getByTestId('status').textContent).toBe('found');
  });

  it('reports missing when the card never appears', () => {
    render(<Harness hash="#call-ghost" ids={['other']} />);
    act(() => {
      vi.advanceTimersByTime(9000);
    });
    expect(scrollSpy).not.toHaveBeenCalled();
    expect(screen.getByTestId('status').textContent).toBe('missing');
  });

  it('stays idle for a non-call hash', () => {
    render(<Harness hash="#something" ids={['a']} />);
    expect(scrollSpy).not.toHaveBeenCalled();
    expect(screen.getByTestId('status').textContent).toBe('idle');
  });
});
