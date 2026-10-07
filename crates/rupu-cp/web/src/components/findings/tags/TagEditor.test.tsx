// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { TagEditor } from './TagEditor';

afterEach(cleanup);
const sugg = [{ tag: 'needs-poc', count: 3 }, { tag: 'class:sqli', count: 2 }];

describe('TagEditor', () => {
  it('removes a tag with its ✕', async () => {
    const onRemove = vi.fn().mockResolvedValue(undefined);
    render(<TagEditor tags={['needs-poc']} suggestions={sugg} onAdd={vi.fn()} onRemove={onRemove} />);
    fireEvent.click(screen.getByRole('button', { name: 'Remove tag needs-poc' }));
    await waitFor(() => expect(onRemove).toHaveBeenCalledWith('needs-poc'));
  });
  it('adds a typed tag, normalized, and offers in-use tags not already present', async () => {
    const onAdd = vi.fn().mockResolvedValue(undefined);
    render(<TagEditor tags={['needs-poc']} suggestions={sugg} onAdd={onAdd} onRemove={vi.fn()} />);
    fireEvent.click(screen.getByRole('button', { name: 'tag' }));
    expect(screen.queryByRole('option', { name: /needs-poc/ })).toBeNull();
    expect(screen.getByRole('option', { name: /class:sqli/ })).toBeInTheDocument();
    fireEvent.change(screen.getByRole('combobox', { name: 'Add tag' }), { target: { value: ' Triaged ' } });
    fireEvent.keyDown(screen.getByRole('combobox', { name: 'Add tag' }), { key: 'Enter' });
    await waitFor(() => expect(onAdd).toHaveBeenCalledWith('triaged'));
  });
  it('rejects an invalid tag without calling onAdd', () => {
    const onAdd = vi.fn();
    render(<TagEditor tags={[]} suggestions={[]} onAdd={onAdd} onRemove={vi.fn()} />);
    fireEvent.click(screen.getByRole('button', { name: 'tag' }));
    fireEvent.change(screen.getByRole('combobox', { name: 'Add tag' }), { target: { value: 'not ok' } });
    fireEvent.keyDown(screen.getByRole('combobox', { name: 'Add tag' }), { key: 'Enter' });
    expect(onAdd).not.toHaveBeenCalled();
    expect(screen.getByRole('alert')).toHaveTextContent('invalid tag');
  });
  it('shows a server error from onAdd', async () => {
    render(<TagEditor tags={[]} suggestions={[]} onAdd={vi.fn().mockRejectedValue(new Error('too many tags'))} onRemove={vi.fn()} />);
    fireEvent.click(screen.getByRole('button', { name: 'tag' }));
    fireEvent.change(screen.getByRole('combobox', { name: 'Add tag' }), { target: { value: 'x' } });
    fireEvent.keyDown(screen.getByRole('combobox', { name: 'Add tag' }), { key: 'Enter' });
    expect(await screen.findByRole('alert')).toHaveTextContent('too many tags');
  });
  it('keeps the typed text when onAdd fails', async () => {
    render(<TagEditor tags={[]} suggestions={[]} onAdd={vi.fn().mockRejectedValue(new Error('too many tags'))} onRemove={vi.fn()} />);
    fireEvent.click(screen.getByRole('button', { name: 'tag' }));
    const box = screen.getByRole('combobox', { name: 'Add tag' });
    fireEvent.change(box, { target: { value: 'triaged' } });
    fireEvent.keyDown(box, { key: 'Enter' });
    expect(await screen.findByRole('alert')).toHaveTextContent('too many tags');
    expect(screen.getByRole('combobox', { name: 'Add tag' })).toHaveValue('triaged');
  });
  it('closes the input and shows a note when a saved add has one', async () => {
    const onAdd = vi.fn().mockResolvedValue({ note: "Saved, but the page couldn't refresh: offline" });
    render(<TagEditor tags={[]} suggestions={[]} onAdd={onAdd} onRemove={vi.fn()} />);
    fireEvent.click(screen.getByRole('button', { name: 'tag' }));
    const box = screen.getByRole('combobox', { name: 'Add tag' });
    fireEvent.change(box, { target: { value: 'triaged' } });
    fireEvent.keyDown(box, { key: 'Enter' });
    expect(await screen.findByRole('status')).toHaveTextContent("Saved, but the page couldn't refresh: offline");
    expect(screen.queryByRole('alert')).toBeNull();
    expect(screen.queryByRole('combobox', { name: 'Add tag' })).toBeNull();
  });
  it("shows a saved add's partial error and still closes the input", async () => {
    const onAdd = vi.fn().mockResolvedValue({ error: "other's tags couldn't be changed: locked." });
    render(<TagEditor tags={[]} suggestions={[]} onAdd={onAdd} onRemove={vi.fn()} />);
    fireEvent.click(screen.getByRole('button', { name: 'tag' }));
    const box = screen.getByRole('combobox', { name: 'Add tag' });
    fireEvent.change(box, { target: { value: 'triaged' } });
    fireEvent.keyDown(box, { key: 'Enter' });
    expect(await screen.findByRole('alert')).toHaveTextContent("other's tags couldn't be changed: locked.");
    expect(screen.queryByRole('combobox', { name: 'Add tag' })).toBeNull();
  });
  it('ignores a repeat submit while onAdd is pending', async () => {
    let resolve!: () => void;
    const onAdd = vi.fn().mockImplementation(() => new Promise<void>((r) => { resolve = r; }));
    render(<TagEditor tags={[]} suggestions={[]} onAdd={onAdd} onRemove={vi.fn()} />);
    fireEvent.click(screen.getByRole('button', { name: 'tag' }));
    const box = screen.getByRole('combobox', { name: 'Add tag' });
    fireEvent.change(box, { target: { value: 'triaged' } });
    fireEvent.keyDown(box, { key: 'Enter' });
    fireEvent.keyDown(box, { key: 'Enter' });
    expect(onAdd).toHaveBeenCalledTimes(1);
    expect(screen.queryByRole('alert')).toBeNull();
    resolve();
    await waitFor(() => expect(screen.queryByRole('combobox', { name: 'Add tag' })).toBeNull());
    expect(onAdd).toHaveBeenCalledTimes(1);
  });
  it('is read-only with a reason when disabled', () => {
    render(<TagEditor tags={['x']} suggestions={[]} disabledReason="this project's tags couldn't be read" onAdd={vi.fn()} onRemove={vi.fn()} />);
    expect(screen.queryByRole('button', { name: 'Remove tag x' })).toBeNull();
    expect(screen.getByText('tags read-only')).toHaveAttribute('title', "this project's tags couldn't be read");
  });
});
