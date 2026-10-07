// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { useCustomerScope } from '../../lib/customerScope';
import { scopedEntry, withCustomerScope } from '../../lib/customerScopeTestUtils';
import { ScopeChip } from './ScopeChip';

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

function ScopeProbe() {
  return <span data-testid="scope">{String(useCustomerScope().scope)}</span>;
}

function mount(scope: string | null) {
  return render(
    <MemoryRouter initialEntries={[scope ? scopedEntry(scope) : '/']}>
      {withCustomerScope(
        <>
          <ScopeChip />
          <ScopeProbe />
        </>,
      )}
    </MemoryRouter>,
  );
}

describe('ScopeChip', () => {
  it('renders nothing when unscoped', () => {
    mount(null);
    expect(screen.queryByRole('button', { name: 'Clear customer scope' })).not.toBeInTheDocument();
  });

  it('shows the scoped customer and clears the scope on ×', async () => {
    mount('acme');
    expect(await screen.findByText('Acme')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Clear customer scope' }));
    await waitFor(() => expect(screen.getByTestId('scope')).toHaveTextContent('null'));
    expect(screen.queryByText('Acme')).not.toBeInTheDocument();
  });

  it('says "Unassigned" for the none scope', () => {
    mount('none');
    expect(screen.getByText('Unassigned')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Clear customer scope' })).toBeInTheDocument();
  });
});
