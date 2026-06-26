// @vitest-environment jsdom
import { afterEach, describe, it, expect, vi } from 'vitest';
import '@testing-library/jest-dom/vitest';
import { render, screen, fireEvent, cleanup } from '@testing-library/react';
import CollapsibleRow from './CollapsibleRow';

afterEach(() => cleanup());

describe('CollapsibleRow', () => {
  it('renders children only when open', () => {
    const { rerender } = render(
      <CollapsibleRow open={false} onToggle={() => {}} header={<span>Head</span>}>
        <span>Body</span>
      </CollapsibleRow>,
    );
    expect(screen.getByText('Head')).toBeInTheDocument();
    expect(screen.queryByText('Body')).not.toBeInTheDocument();

    rerender(
      <CollapsibleRow open={true} onToggle={() => {}} header={<span>Head</span>}>
        <span>Body</span>
      </CollapsibleRow>,
    );
    expect(screen.getByText('Body')).toBeInTheDocument();
  });

  it('calls onToggle when the header is clicked', () => {
    const onToggle = vi.fn();
    render(
      <CollapsibleRow open={false} onToggle={onToggle} header={<span>Head</span>}>
        <span>Body</span>
      </CollapsibleRow>,
    );
    fireEvent.click(screen.getByText('Head'));
    expect(onToggle).toHaveBeenCalledOnce();
  });
});
