// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { cleanup, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it } from 'vitest';
import { FidelityBadge, FIDELITY_TITLE } from './FidelityBadge';
import type { Fidelity } from '../../lib/netflow';

afterEach(() => {
  cleanup();
});

describe('FidelityBadge', () => {
  it('renders the socket label with its title', () => {
    render(<FidelityBadge fidelity="socket" />);
    const el = screen.getByText('socket');
    expect(el).toBeInTheDocument();
    expect(el).toHaveAttribute('title', FIDELITY_TITLE.socket);
    expect(FIDELITY_TITLE.socket).toBe(
      'Socket — process, remote IP:port, bytes and timing observed from the OS socket table; no URL, method or HTTP status.',
    );
  });

  it('uses a tone distinct from every other fidelity', () => {
    const levels: Fidelity[] = ['coarse', 'socket', 'http', 'full'];
    const classes = levels.map((f) => {
      const { container, unmount } = render(<FidelityBadge fidelity={f} />);
      const cls = (container.firstElementChild as HTMLElement).className;
      unmount();
      return cls;
    });
    expect(new Set(classes).size).toBe(levels.length);
  });
});
