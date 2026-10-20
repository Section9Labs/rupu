// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { afterEach, describe, expect, it } from 'vitest';
import { cleanup, render, screen } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { PageHeader } from './PageHeader';

afterEach(cleanup);

const wrap = (ui: React.ReactNode) => render(<MemoryRouter>{ui}</MemoryRouter>);

describe('PageHeader', () => {
  it('renders the title', () => {
    wrap(<PageHeader title="Overview" />);
    expect(screen.getByRole('heading', { name: 'Overview' })).toBeInTheDocument();
  });

  it('renders a back link when given', () => {
    wrap(<PageHeader title="run" back={{ label: 'Agent runs', to: '/runs' }} />);
    const back = screen.getByRole('link', { name: /Agent runs/ });
    expect(back).toHaveAttribute('href', '/runs');
  });

  it('omits the subtitle node entirely when absent', () => {
    const { container } = wrap(<PageHeader title="Overview" />);
    expect(container.querySelector('p')).toBeNull();
  });

  it('renders the subtitle and actions when given', () => {
    wrap(
      <PageHeader
        title="Overview"
        subtitle="All customers · updated 8s ago"
        actions={<button type="button">Usage</button>}
      />,
    );
    expect(screen.getByText('All customers · updated 8s ago')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Usage' })).toBeInTheDocument();
  });
});
