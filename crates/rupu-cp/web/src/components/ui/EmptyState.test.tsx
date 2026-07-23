// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { afterEach, describe, expect, it } from 'vitest';
import { cleanup, render, screen } from '@testing-library/react';
import { EmptyState } from './EmptyState';

afterEach(cleanup);

describe('EmptyState', () => {
  it('renders a bold title', () => {
    render(<EmptyState title="No runs yet" />);
    const title = screen.getByText('No runs yet');
    expect(title.className).toMatch(/font-semibold/);
  });

  it('renders an optional dim hint', () => {
    render(<EmptyState title="No runs yet" hint="Trigger one from a workflow." />);
    const hint = screen.getByText('Trigger one from a workflow.');
    expect(hint.className).toMatch(/text-ink-mute/);
  });

  it('omits the hint when not given', () => {
    render(<EmptyState title="No runs yet" />);
    expect(screen.queryByText(/Trigger one/)).toBeNull();
  });

  it('renders an optional action', () => {
    render(<EmptyState title="No runs yet" action={<button type="button">Create</button>} />);
    expect(screen.getByRole('button', { name: 'Create' })).toBeInTheDocument();
  });

  it('uses a dashed, rounded, centered box', () => {
    const { container } = render(<EmptyState title="No runs yet" />);
    const box = container.firstElementChild as HTMLElement;
    expect(box.className).toMatch(/border-dashed/);
    expect(box.className).toMatch(/rounded-lg/);
    expect(box.className).toMatch(/text-center/);
  });
});
