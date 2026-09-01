// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { render, screen, cleanup, fireEvent } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import FilterChips from './FilterChips';
import { EMPTY_NETFLOW_FILTERS } from '../../../lib/netflow';

afterEach(() => {
  cleanup();
});

const noop = () => {};

describe('FilterChips', () => {
  it('renders nothing when no filter and no window is active', () => {
    const { container } = render(
      <FilterChips
        filters={EMPTY_NETFLOW_FILTERS}
        orgLabel={(k) => k}
        windowApplied={false}
        onRemove={noop}
        onClearWindow={noop}
        onClearAll={noop}
      />,
    );
    expect(container).toBeEmptyDOMElement();
  });

  it('renders one labeled chip per active filter, org chips by display label', () => {
    render(
      <FilterChips
        filters={{
          workflows: ['review-wf'],
          origins: ['provider:anthropic'],
          orgs: ['as13335'],
          hosts: ['api.anthropic.com:443'],
        }}
        orgLabel={(k) => (k === 'as13335' ? 'Cloudflare' : k)}
        windowApplied
        onRemove={noop}
        onClearWindow={noop}
        onClearAll={noop}
      />,
    );
    expect(screen.getByText('workflow:review-wf')).toBeInTheDocument();
    expect(screen.getByText('provider:anthropic')).toBeInTheDocument();
    expect(screen.getByText('net:Cloudflare')).toBeInTheDocument();
    expect(screen.getByText('api.anthropic.com:443')).toBeInTheDocument();
    expect(screen.getByText('window')).toBeInTheDocument();
  });

  it('clicking a chip removes exactly that filter; window chip clears the window', () => {
    const onRemove = vi.fn();
    const onClearWindow = vi.fn();
    render(
      <FilterChips
        filters={{ ...EMPTY_NETFLOW_FILTERS, hosts: ['api.anthropic.com:443'] }}
        orgLabel={(k) => k}
        windowApplied
        onRemove={onRemove}
        onClearWindow={onClearWindow}
        onClearAll={noop}
      />,
    );
    // Accessible names carry the dim-prefixed id, not the bare label —
    // labels are not unique (two ASNs can share one org name).
    fireEvent.click(
      screen.getByRole('button', { name: /remove filter host:api.anthropic.com/i }),
    );
    expect(onRemove).toHaveBeenCalledWith('hosts', 'api.anthropic.com:443');

    fireEvent.click(screen.getByRole('button', { name: /remove filter window/i }));
    expect(onClearWindow).toHaveBeenCalled();
  });

  it('two orgs sharing one display label stay distinct chips (id-keyed, id-labelled)', () => {
    // ASN org names are not unique per ASN — e.g. one operator announcing
    // several ASNs under one org string. Keys and accessible names carry
    // the `as<number>` id so the chips never collide.
    const onRemove = vi.fn();
    render(
      <FilterChips
        filters={{ ...EMPTY_NETFLOW_FILTERS, orgs: ['as15169', 'as396982'] }}
        orgLabel={() => 'Google LLC'}
        windowApplied={false}
        onRemove={onRemove}
        onClearWindow={noop}
        onClearAll={noop}
      />,
    );
    expect(screen.getAllByText('net:Google LLC').length).toBe(2);
    fireEvent.click(screen.getByRole('button', { name: 'Remove filter org:as396982' }));
    expect(onRemove).toHaveBeenCalledWith('orgs', 'as396982');
    expect(onRemove).not.toHaveBeenCalledWith('orgs', 'as15169');
  });

  it('Clear all clears everything at once', () => {
    const onClearAll = vi.fn();
    render(
      <FilterChips
        filters={{ ...EMPTY_NETFLOW_FILTERS, workflows: ['a'] }}
        orgLabel={(k) => k}
        windowApplied={false}
        onRemove={noop}
        onClearWindow={noop}
        onClearAll={onClearAll}
      />,
    );
    fireEvent.click(screen.getByRole('button', { name: /clear all/i }));
    expect(onClearAll).toHaveBeenCalled();
  });
});
