// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { FilterPills } from './FilterPills';

const OPTIONS = [
  { value: 'all', label: 'All' },
  { value: 'manual', label: 'Manual' },
  { value: 'cron', label: 'Cron' },
];

afterEach(cleanup);

describe('FilterPills', () => {
  it('renders the caller-supplied options verbatim (does not inject its own All)', () => {
    render(<FilterPills options={OPTIONS} value="all" onChange={vi.fn()} />);
    expect(screen.getAllByRole('button')).toHaveLength(3);
    expect(screen.getByRole('button', { name: 'All' })).toBeInTheDocument();
  });

  it('brand-fills the active pill and leaves inactive pills neutral', () => {
    render(<FilterPills options={OPTIONS} value="cron" onChange={vi.fn()} />);
    const cron = screen.getByRole('button', { name: 'Cron' });
    const all = screen.getByRole('button', { name: 'All' });
    expect(cron).toHaveAttribute('aria-pressed', 'true');
    expect(cron.className).toMatch(/bg-brand-600/);
    expect(cron.className).toMatch(/border-brand-600/);
    expect(cron.className).toMatch(/text-white/);
    expect(all).toHaveAttribute('aria-pressed', 'false');
    expect(all.className).not.toMatch(/bg-brand-600/);
  });

  it('is single-select: clicking a pill reports its value via onChange', () => {
    const onChange = vi.fn();
    render(<FilterPills options={OPTIONS} value="all" onChange={onChange} />);
    fireEvent.click(screen.getByRole('button', { name: 'Manual' }));
    expect(onChange).toHaveBeenCalledTimes(1);
    expect(onChange).toHaveBeenCalledWith('manual');
  });

  it('renders an optional tiny uppercase group label', () => {
    render(<FilterPills label="Trigger" options={OPTIONS} value="all" onChange={vi.fn()} />);
    const label = screen.getByText('Trigger');
    expect(label.className).toMatch(/uppercase/);
  });

  it('omits the group label entirely when none is given', () => {
    render(<FilterPills options={OPTIONS} value="all" onChange={vi.fn()} />);
    expect(screen.queryByText('Trigger')).toBeNull();
  });

  it('never lets a pill label wrap', () => {
    render(<FilterPills options={OPTIONS} value="all" onChange={vi.fn()} />);
    expect(screen.getByRole('button', { name: 'All' }).className).toMatch(/whitespace-nowrap/);
  });
});
