// @vitest-environment jsdom
// Composite — the generic Segmented-tab wrapper interim v2 destination pages
// use to carry the 7-leaf IA (Activity/Security/Library/Fleet) over the
// existing v1 page bodies. The active tab lives in `?tab=` so redirects from
// old v1 routes can deep-link a specific tab (e.g. `/activity?tab=agents`)
// and the resulting URL stays shareable/bookmarkable.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect } from 'vitest';
import { render, screen, cleanup, fireEvent } from '@testing-library/react';
import { MemoryRouter, Routes, Route, useLocation } from 'react-router-dom';
import { Composite } from './Composite';

afterEach(() => {
  cleanup();
});

function LocationSpy() {
  const loc = useLocation();
  return <div data-testid="loc">{loc.pathname + loc.search}</div>;
}

function renderComposite(initialEntry = '/x') {
  return render(
    <MemoryRouter initialEntries={[initialEntry]}>
      <LocationSpy />
      <Routes>
        <Route
          path="/x"
          element={
            <Composite
              title="Example"
              defaultTab="a"
              tabs={[
                { value: 'a', label: 'alpha', element: <div>A</div> },
                { value: 'b', label: 'beta', element: <div>B</div> },
              ]}
            />
          }
        />
      </Routes>
    </MemoryRouter>,
  );
}

describe('Composite', () => {
  it('renders the title and every tab as a segmented option', () => {
    renderComposite();
    expect(screen.getByText('Example')).toBeInTheDocument();
    const group = screen.getByRole('group', { name: 'Example sections' });
    expect(screen.getByRole('button', { name: 'alpha' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'beta' })).toBeInTheDocument();
    void group;
  });

  it('with no ?tab= param, renders the defaultTab body', () => {
    renderComposite('/x');
    expect(screen.getByText('A')).toBeInTheDocument();
    expect(screen.queryByText('B')).not.toBeInTheDocument();
  });

  it('an unknown ?tab= value falls back to defaultTab', () => {
    renderComposite('/x?tab=nonsense');
    expect(screen.getByText('A')).toBeInTheDocument();
    expect(screen.queryByText('B')).not.toBeInTheDocument();
  });

  it('a known ?tab= param selects that tab body on first render', () => {
    renderComposite('/x?tab=b');
    expect(screen.getByText('B')).toBeInTheDocument();
    expect(screen.queryByText('A')).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'beta' })).toHaveAttribute('aria-pressed', 'true');
  });

  it('clicking a segment updates ?tab= and swaps the body', () => {
    renderComposite('/x?tab=b');
    fireEvent.click(screen.getByRole('button', { name: 'alpha' }));
    expect(screen.getByText('A')).toBeInTheDocument();
    expect(screen.queryByText('B')).not.toBeInTheDocument();
    expect(screen.getByTestId('loc')).toHaveTextContent('/x?tab=a');
  });
});
