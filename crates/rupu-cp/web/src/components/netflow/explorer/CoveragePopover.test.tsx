// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { render, screen, cleanup, fireEvent } from '@testing-library/react';
import { afterEach, describe, expect, it } from 'vitest';
import CoveragePopover from './CoveragePopover';
import { FIDELITY_TITLE } from '../FidelityBadge';

afterEach(() => {
  cleanup();
});

describe('CoveragePopover', () => {
  it('is closed until the pill is clicked, then shows the single-sourced disclosure', () => {
    render(<CoveragePopover scope="global" droppedTotal={0} />);
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: /coverage & gaps/i }));
    expect(screen.getByRole('dialog')).toBeInTheDocument();
    // disclosureText — authored once in ScopeDisclosure.tsx.
    expect(screen.getByText(/git2 clones/i)).toBeInTheDocument();
  });

  it('appends the sub-agent folding note at run scope only', () => {
    const { unmount } = render(<CoveragePopover scope="run" droppedTotal={0} />);
    fireEvent.click(screen.getByRole('button', { name: /coverage & gaps/i }));
    expect(screen.getByText(/sub-agents this run dispatched/i)).toBeInTheDocument();
    unmount();

    render(<CoveragePopover scope="project" droppedTotal={0} />);
    fireEvent.click(screen.getByRole('button', { name: /coverage & gaps/i }));
    expect(screen.queryByText(/sub-agents this run dispatched/i)).not.toBeInTheDocument();
  });

  it('states capture-unavailable on the pill and its reason in the popover at run scope', () => {
    render(
      <CoveragePopover
        scope="run"
        droppedTotal={0}
        capture={{ state: 'unavailable', reason: 'lsof not permitted', notes: [] }}
      />,
    );
    const pill = screen.getByRole('button', { name: /coverage & gaps/i });
    expect(pill).toHaveTextContent(/subprocess capture unavailable/i);
    fireEvent.click(pill);
    expect(screen.getByText(/capture unavailable on this run: lsof not permitted/i)).toBeInTheDocument();
  });

  it('carries the fidelity legend from FIDELITY_TITLE for all four levels', () => {
    render(<CoveragePopover scope="global" droppedTotal={0} />);
    fireEvent.click(screen.getByRole('button', { name: /coverage & gaps/i }));
    expect(screen.getByText(/exact request and response metadata/i)).toBeInTheDocument();
    expect(screen.getByText(/not observable for this connector/i)).toBeInTheDocument();
    expect(screen.getByText(/frame-level capture/i)).toBeInTheDocument();
    expect(screen.getByText(FIDELITY_TITLE.socket)).toBeInTheDocument();
  });

  it('states the dropped accounting (same wording as the banner) only when loss occurred', () => {
    const { unmount } = render(<CoveragePopover scope="global" droppedTotal={7} />);
    fireEvent.click(screen.getByRole('button', { name: /coverage & gaps/i }));
    expect(screen.getByText(/7 flows dropped across the full history/i)).toBeInTheDocument();
    unmount();

    render(<CoveragePopover scope="global" droppedTotal={0} />);
    fireEvent.click(screen.getByRole('button', { name: /coverage & gaps/i }));
    expect(screen.queryByText(/dropped/i)).not.toBeInTheDocument();
  });

  it('closes on Escape', () => {
    render(<CoveragePopover scope="global" droppedTotal={0} />);
    fireEvent.click(screen.getByRole('button', { name: /coverage & gaps/i }));
    expect(screen.getByRole('dialog')).toBeInTheDocument();

    fireEvent.keyDown(document, { key: 'Escape' });
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  });
});
