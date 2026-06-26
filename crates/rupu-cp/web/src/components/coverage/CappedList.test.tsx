// @vitest-environment jsdom
import { afterEach, describe, it, expect } from 'vitest';
import '@testing-library/jest-dom/vitest';
import { render, screen, fireEvent, cleanup } from '@testing-library/react';
import CappedList from './CappedList';

afterEach(() => cleanup());

describe('CappedList', () => {
  it('shows only the first `cap` items until expanded', () => {
    const items = Array.from({ length: 5 }, (_, i) => `file-${i}.rs`);
    render(<CappedList items={items} cap={2} />);
    expect(screen.getByText('file-0.rs')).toBeInTheDocument();
    expect(screen.getByText('file-1.rs')).toBeInTheDocument();
    expect(screen.queryByText('file-2.rs')).not.toBeInTheDocument();

    fireEvent.click(screen.getByText(/show all 5/i));
    expect(screen.getByText('file-2.rs')).toBeInTheDocument();
    expect(screen.getByText('file-4.rs')).toBeInTheDocument();
  });

  it('shows no toggle when items fit under the cap', () => {
    render(<CappedList items={['only.rs']} cap={10} />);
    expect(screen.getByText('only.rs')).toBeInTheDocument();
    expect(screen.queryByText(/show all/i)).not.toBeInTheDocument();
  });
});
