// @vitest-environment jsdom
import { act, render } from '@testing-library/react';
import { RouterProvider, createMemoryRouter } from 'react-router-dom';
import { describe, expect, it } from 'vitest';
import { readTab, useTabParam, withTab } from './tabParam';

const TABS = ['a', 'b', 'c'] as const;

describe('readTab', () => {
  it('reads a tab the page has', () => {
    expect(readTab('?host=h1&tab=b', TABS, 'a')).toBe('b');
  });

  it('falls back when the param is absent', () => {
    expect(readTab('?host=h1', TABS, 'c')).toBe('c');
  });

  it('falls back when the param names no tab', () => {
    expect(readTab('?tab=zzz', TABS, 'a')).toBe('a');
  });
});

describe('withTab', () => {
  it('adds the tab and keeps the other params', () => {
    expect(withTab('?host=h1', 'b')).toBe('?host=h1&tab=b');
  });

  it('replaces an existing tab in place', () => {
    expect(withTab('?tab=a&host=h1', 'c')).toBe('?tab=c&host=h1');
  });

  it('writes into an empty search', () => {
    expect(withTab('', 'a')).toBe('?tab=a');
  });
});

let setTabRef: ((tab: (typeof TABS)[number]) => void) | null = null;

function Probe() {
  const [tab, setTab] = useTabParam(TABS, 'a');
  setTabRef = setTab;
  return <p>tab={tab}</p>;
}

describe('useTabParam', () => {
  it('rewrites the param by replace, keeping the hash and the router state', () => {
    const router = createMemoryRouter([{ path: '/x', element: <Probe /> }], {
      initialEntries: [{ pathname: '/x', search: '?host=h1', hash: '#msg-abc', state: { from: '/findings/f1' } }],
    });
    const { getByText } = render(<RouterProvider router={router} />);
    expect(getByText('tab=a')).toBeTruthy();

    act(() => setTabRef!('b'));

    expect(getByText('tab=b')).toBeTruthy();
    expect(router.state.location.pathname).toBe('/x');
    expect(router.state.location.search).toBe('?host=h1&tab=b');
    expect(router.state.location.hash).toBe('#msg-abc');
    expect(router.state.location.state).toEqual({ from: '/findings/f1' });
    expect(router.state.historyAction).toBe('REPLACE');
  });
});
