// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { useState } from 'react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { QueryBar } from './QueryBar';
import { FINDING_FIELDS } from '../../lib/findingQuery/fields';

const facets = { tag: [{ value: 'needs-poc', count: 3 }, { value: 'class:sqli', count: 2 }] };

function Harness({ initial = '', spy = vi.fn() }: { initial?: string; spy?: (q: string) => void }) {
  const [q, setQ] = useState(initial);
  return (
    <QueryBar
      value={q}
      onChange={(next) => {
        spy(next);
        setQ(next);
      }}
      fields={FINDING_FIELDS}
      facets={facets}
      label="Filter findings"
    />
  );
}

afterEach(cleanup);

const input = () => screen.getByRole('combobox', { name: 'Filter findings' });

describe('QueryBar', () => {
  it('renders committed tokens as chips', () => {
    render(<Harness initial="severity>=high -tag:noise" />);
    expect(screen.getByText('severity ≥ high')).toBeInTheDocument();
    expect(screen.getByText('not tag: noise')).toBeInTheDocument();
  });

  it('accepts a key then a value from the spotlight with the keyboard', () => {
    const spy = vi.fn();
    render(<Harness spy={spy} />);
    fireEvent.focus(input());
    fireEvent.change(input(), { target: { value: 'ta' } });
    fireEvent.keyDown(input(), { key: 'ArrowDown' });
    fireEvent.keyDown(input(), { key: 'Enter' });
    expect(input()).toHaveValue('tag:');
    fireEvent.keyDown(input(), { key: 'ArrowDown' });
    fireEvent.keyDown(input(), { key: 'Enter' });
    expect(spy).toHaveBeenLastCalledWith('tag:needs-poc');
    expect(input()).toHaveValue('');
  });

  it('ArrowUp from no selection wraps to the last option', () => {
    render(<Harness />);
    fireEvent.change(input(), { target: { value: 'tag:' } });
    fireEvent.keyDown(input(), { key: 'ArrowUp' });
    const opts = screen.getAllByRole('option');
    expect(opts[opts.length - 1]).toHaveAttribute('aria-selected', 'true');
  });

  it('commits typed free text on Enter', () => {
    const spy = vi.fn();
    render(<Harness spy={spy} />);
    fireEvent.change(input(), { target: { value: 'sql' } });
    fireEvent.keyDown(input(), { key: 'Enter' });
    expect(spy).toHaveBeenLastCalledWith('sql');
  });

  it('refuses an invalid draft and shows why', () => {
    const spy = vi.fn();
    render(<Harness spy={spy} />);
    fireEvent.change(input(), { target: { value: 'sevrity:high' } });
    fireEvent.keyDown(input(), { key: 'Enter' });
    expect(spy).not.toHaveBeenCalled();
    expect(screen.getByRole('alert')).toHaveTextContent('unknown key `sevrity`');
  });

  it('Backspace on an empty draft pops the last chip back into the draft', () => {
    const spy = vi.fn();
    render(<Harness initial="tag:a tag:b" spy={spy} />);
    fireEvent.keyDown(input(), { key: 'Backspace' });
    expect(spy).toHaveBeenLastCalledWith('tag:a');
    expect(input()).toHaveValue('tag:b');
  });

  it('removes a chip with its ✕ button', () => {
    const spy = vi.fn();
    render(<Harness initial="tag:a tag:b" spy={spy} />);
    fireEvent.click(screen.getByRole('button', { name: 'Remove tag: a' }));
    expect(spy).toHaveBeenLastCalledWith('tag:b');
  });

  it("a negated chip's ✕ names it with its not", () => {
    const spy = vi.fn();
    render(<Harness initial="-tag:a tag:a" spy={spy} />);
    expect(screen.getByRole('button', { name: 'Remove tag: a' })).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Remove not tag: a' }));
    expect(spy).toHaveBeenLastCalledWith('tag:a');
  });

  it('marks an invalid chip from the URL in red with its reason', () => {
    render(<Harness initial="sevrity:high" />);
    expect(screen.getByTitle(/unknown key/)).toHaveClass('ring-err/40');
  });

  it('/ focuses the bar from anywhere outside an editable field', () => {
    render(<Harness />);
    fireEvent.keyDown(document.body, { key: '/' });
    expect(input()).toHaveFocus();
  });

  it('has no active row after accepting a key: Enter commits the draft, not a value', () => {
    const spy = vi.fn();
    render(<Harness spy={spy} />);
    fireEvent.change(input(), { target: { value: 'ta' } });
    fireEvent.keyDown(input(), { key: 'ArrowDown' });
    fireEvent.keyDown(input(), { key: 'Enter' });
    expect(input()).toHaveValue('tag:');
    expect(input()).not.toHaveAttribute('aria-activedescendant');
    fireEvent.keyDown(input(), { key: 'Enter' });
    expect(spy).not.toHaveBeenCalled();
    expect(screen.getByRole('alert')).toBeInTheDocument();
  });

  it('Escape clears the draft first', () => {
    render(<Harness />);
    fireEvent.change(input(), { target: { value: 'sql' } });
    fireEvent.keyDown(input(), { key: 'Escape' });
    expect(input()).toHaveValue('');
  });
});
