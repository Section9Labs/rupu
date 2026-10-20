// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { afterEach, describe, expect, it } from 'vitest';
import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { MemoryRouter, Route, Routes, useLocation } from 'react-router-dom';
import { ReportLink } from './ReportLink';

afterEach(cleanup);

function Probe() {
  const { pathname, search, state } = useLocation();
  return <pre data-testid="probe">{JSON.stringify({ pathname, search, state })}</pre>;
}

function renderFrom(entry: { pathname: string; search?: string; hash?: string; state?: unknown }) {
  return render(
    <MemoryRouter initialEntries={[entry]}>
      <Routes>
        <Route path="/projects/:wsId/findings" element={<ReportLink id="fnd_1" wsId="ws1">Open full report →</ReportLink>} />
        <Route path="/findings/:id" element={<Probe />} />
      </Routes>
    </MemoryRouter>,
  );
}

describe('ReportLink', () => {
  it('links to the report, naming the workspace', () => {
    renderFrom({ pathname: '/projects/ws1/findings' });
    expect(screen.getByRole('link', { name: 'Open full report →' })).toHaveAttribute('href', '/findings/fnd_1?ws_id=ws1');
  });

  it('carries the page it was clicked on, with its query, hash and state, so the report can return there', () => {
    const originState = { back: { to: '/projects', label: 'Back' } };
    renderFrom({ pathname: '/projects/ws1/findings', search: '?q=sev%3Ahigh', hash: '#row-3', state: originState });
    fireEvent.click(screen.getByRole('link', { name: 'Open full report →' }));
    expect(JSON.parse(screen.getByTestId('probe').textContent!)).toEqual({
      pathname: '/findings/fnd_1',
      search: '?ws_id=ws1',
      state: { back: { to: '/projects/ws1/findings?q=sev%3Ahigh#row-3', label: 'Project', state: originState } },
    });
  });
});
