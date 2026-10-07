// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { BulkTagBar } from './BulkTagBar';

afterEach(cleanup);

describe('BulkTagBar', () => {
  it('tags the selection with a chosen tag and shows the result', async () => {
    const onApply = vi.fn().mockResolvedValue({ message: 'Tagged 2 findings.', ok: true });
    render(<BulkTagBar count={2} suggestions={[{ tag: 'needs-poc', count: 3 }]} onApply={onApply} onClear={vi.fn()} />);
    expect(screen.getByText('2 selected')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Tag…' }));
    fireEvent.change(screen.getByRole('combobox', { name: 'Tag selected findings' }), { target: { value: 'needs-poc' } });
    fireEvent.keyDown(screen.getByRole('combobox', { name: 'Tag selected findings' }), { key: 'Enter' });
    expect(await screen.findByRole('status')).toHaveTextContent('Tagged 2 findings.');
    expect(onApply).toHaveBeenCalledWith('add', 'needs-poc');
  });
  it('untag mode and clear', async () => {
    const onApply = vi.fn().mockResolvedValue({ message: "Untagged 1 finding. billing-api's tags couldn't be changed: x.", ok: false });
    const onClear = vi.fn();
    render(<BulkTagBar count={3} suggestions={[]} onApply={onApply} onClear={onClear} />);
    fireEvent.click(screen.getByRole('button', { name: 'Untag…' }));
    fireEvent.change(screen.getByRole('combobox', { name: 'Untag selected findings' }), { target: { value: 'x' } });
    fireEvent.keyDown(screen.getByRole('combobox', { name: 'Untag selected findings' }), { key: 'Enter' });
    expect(await screen.findByRole('alert')).toHaveTextContent("billing-api's tags couldn't be changed");
    fireEvent.click(screen.getByRole('button', { name: 'Clear' }));
    expect(onClear).toHaveBeenCalled();
  });
  it('keeps the typed tag and shows the error when the apply fails', async () => {
    const onApply = vi.fn().mockRejectedValue(new Error('network down'));
    render(<BulkTagBar count={2} suggestions={[]} onApply={onApply} onClear={vi.fn()} />);
    fireEvent.click(screen.getByRole('button', { name: 'Tag…' }));
    const input = screen.getByRole('combobox', { name: 'Tag selected findings' }) as HTMLInputElement;
    fireEvent.change(input, { target: { value: 'triaged' } });
    fireEvent.keyDown(input, { key: 'Enter' });
    expect(await screen.findByRole('alert')).toHaveTextContent('network down');
    // Still in tag mode, with the text intact for a retry.
    const again = screen.getByRole('combobox', { name: 'Tag selected findings' }) as HTMLInputElement;
    expect(again.value).toBe('triaged');
    expect(again).not.toHaveAttribute('readonly');
  });
  it('ignores a repeat Enter while an apply is in flight', async () => {
    let release: (r: { message: string; ok: boolean }) => void = () => {};
    const onApply = vi.fn().mockImplementation(
      () =>
        new Promise((r) => {
          release = r;
        }),
    );
    render(<BulkTagBar count={2} suggestions={[]} onApply={onApply} onClear={vi.fn()} />);
    fireEvent.click(screen.getByRole('button', { name: 'Tag…' }));
    const input = screen.getByRole('combobox', { name: 'Tag selected findings' });
    fireEvent.change(input, { target: { value: 'triaged' } });
    fireEvent.keyDown(input, { key: 'Enter' });
    await waitFor(() => expect(input).toHaveAttribute('readonly'));
    fireEvent.keyDown(input, { key: 'Enter' });
    fireEvent.keyDown(input, { key: 'Enter' });
    expect(onApply).toHaveBeenCalledTimes(1);
    release({ message: 'Tagged 2 findings.', ok: true });
    expect(await screen.findByRole('status')).toHaveTextContent('Tagged 2 findings.');
    expect(onApply).toHaveBeenCalledTimes(1);
  });
});
