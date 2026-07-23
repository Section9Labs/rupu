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

  it('omits the icon slot when not given', () => {
    const { container } = render(<EmptyState title="No runs yet" />);
    expect(container.querySelector('svg')).toBeNull();
  });

  it('renders an optional icon, dim and centered, above the title', () => {
    render(<EmptyState title="No runs yet" icon={<svg data-testid="empty-icon" />} />);
    const icon = screen.getByTestId('empty-icon');
    const wrapper = icon.parentElement as HTMLElement;
    expect(wrapper.className).toMatch(/text-ink-mute/);
    expect(wrapper.className).toMatch(/justify-center/);
  });

  it('accepts a ReactNode hint (e.g. with a mono-styled path segment)', () => {
    render(
      <EmptyState
        title="No workflows found"
        hint={
          <>
            Add workflow YAML under <span className="font-mono">.rupu/workflows/</span> to populate
            this library.
          </>
        }
      />,
    );
    const mono = screen.getByText('.rupu/workflows/');
    expect(mono.className).toMatch(/font-mono/);
  });
});
