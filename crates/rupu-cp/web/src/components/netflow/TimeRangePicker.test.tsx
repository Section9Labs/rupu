// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { render, screen, fireEvent, cleanup } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import TimeRangePicker, { toNetflowRange, type TimeRangeValue } from './TimeRangePicker';

afterEach(() => {
  cleanup();
});

// Fixed instant so relative-preset math (Last hour / 24h / 7d) is
// deterministic rather than depending on the wall clock at test time.
const NOW = () => new Date('2026-08-17T15:00:00.000Z');

describe('TimeRangePicker', () => {
  it('offers relative presets and defaults to all', () => {
    render(<TimeRangePicker value={{ preset: 'all' }} onChange={vi.fn()} now={NOW} />);
    expect(screen.getByRole('button', { name: /last hour/i })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /24h/i })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /7d/i })).toBeInTheDocument();
    // 'All' is the active preset for the default value.
    expect(screen.getByRole('button', { name: /^all$/i })).toHaveAttribute('aria-pressed', 'true');
  });

  it('emits a relative RFC 3339 lower bound when a preset is clicked', () => {
    const onChange = vi.fn();
    render(<TimeRangePicker value={{ preset: 'all' }} onChange={onChange} now={NOW} />);
    fireEvent.click(screen.getByRole('button', { name: /last hour/i }));
    expect(onChange).toHaveBeenCalledWith({ preset: 'hour', from: '2026-08-17T14:00:00.000Z' });
  });

  it('emits an absolute from/to when a custom range is entered and applied', () => {
    const onChange = vi.fn();
    render(<TimeRangePicker value={{ preset: 'all' }} onChange={onChange} now={NOW} />);

    fireEvent.click(screen.getByRole('button', { name: /^custom$/i }));
    fireEvent.change(screen.getByLabelText(/^from$/i), { target: { value: '2026-08-10T09:30' } });
    fireEvent.change(screen.getByLabelText(/^to$/i), { target: { value: '2026-08-11T09:30' } });
    fireEvent.click(screen.getByRole('button', { name: /^apply$/i }));

    expect(onChange).toHaveBeenCalledTimes(1);
    const call = onChange.mock.calls[0][0] as TimeRangeValue;
    expect(call.preset).toBe('custom');
    expect(call.from).toMatch(/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z$/);
    expect(call.to).toMatch(/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z$/);
  });

  it('does not emit bounds for the all preset', () => {
    const onChange = vi.fn();
    // Start from a narrowed value so clicking "All" is an actual change.
    render(
      <TimeRangePicker
        value={{ preset: 'hour', from: '2026-08-17T14:00:00.000Z' }}
        onChange={onChange}
        now={NOW}
      />,
    );
    fireEvent.click(screen.getByRole('button', { name: /^all$/i }));
    expect(onChange).toHaveBeenCalledWith({ preset: 'all' });
    const call = onChange.mock.calls[0][0] as TimeRangeValue;
    expect(call.from).toBeUndefined();
    expect(call.to).toBeUndefined();
  });

  it('does not call onChange on mount', () => {
    const onChange = vi.fn();
    render(<TimeRangePicker value={{ preset: 'all' }} onChange={onChange} now={NOW} />);
    expect(onChange).not.toHaveBeenCalled();
  });

  it('pre-fills the custom inputs from an already-applied custom value on mount', () => {
    // A remount (e.g. RunDetail unmounts the Network tab body on tab
    // switch) must not show blank custom fields while a custom filter is
    // still actually applied — that would read as "did my range get
    // cleared?" when it did not.
    render(
      <TimeRangePicker
        value={{ preset: 'custom', from: '2026-08-10T09:30:00.000Z', to: '2026-08-11T09:30:00.000Z' }}
        onChange={vi.fn()}
        now={NOW}
      />,
    );
    const from = screen.getByLabelText(/^from$/i) as HTMLInputElement;
    const to = screen.getByLabelText(/^to$/i) as HTMLInputElement;
    expect(from.value).not.toBe('');
    expect(to.value).not.toBe('');
  });

  // --- Critical 1 / Important 4 (whole-branch review round 1): the pressed
  // segment and the custom fields must reflect what was actually APPLIED
  // (the `value` prop), never local "which button did the operator just
  // click" navigation state — this control is the operator's only
  // statement of what's currently in effect. ---

  it('does not move the pressed segment to Custom merely by opening the custom editor', () => {
    // Applied value is still 'hour' — clicking "Custom" only reveals the
    // editor; it must not make the control claim "Custom" is applied
    // before anything has actually been submitted via Apply.
    render(
      <TimeRangePicker
        value={{ preset: 'hour', from: '2026-08-17T14:00:00.000Z' }}
        onChange={vi.fn()}
        now={NOW}
      />,
    );
    fireEvent.click(screen.getByRole('button', { name: /^custom$/i }));

    expect(screen.getByRole('button', { name: /last hour/i })).toHaveAttribute(
      'aria-pressed',
      'true',
    );
    expect(screen.getByRole('button', { name: /^custom$/i })).toHaveAttribute(
      'aria-pressed',
      'false',
    );
    // The editor is visible even though its segment isn't "pressed".
    expect(screen.getByLabelText(/^from$/i)).toBeInTheDocument();
  });

  it('only shows Custom as pressed once the applied value itself is custom', () => {
    render(
      <TimeRangePicker
        value={{ preset: 'custom', from: '2026-08-10T09:30:00.000Z', to: '2026-08-11T09:30:00.000Z' }}
        onChange={vi.fn()}
        now={NOW}
      />,
    );
    expect(screen.getByRole('button', { name: /^custom$/i })).toHaveAttribute(
      'aria-pressed',
      'true',
    );
  });

  it('re-syncs the pressed segment and clears the custom editor when the applied value changes externally', () => {
    // Simulates a run switch (RunDetail resets its range state to
    // {preset:'all'} on a new run id without unmounting the picker): the
    // control must drop its own local "Custom is open with these bounds"
    // state and follow the new applied value, not keep showing the
    // previous run's filter.
    const onChange = vi.fn();
    const { rerender } = render(
      <TimeRangePicker
        value={{ preset: 'custom', from: '2026-08-10T09:30:00.000Z', to: '2026-08-11T09:30:00.000Z' }}
        onChange={onChange}
        now={NOW}
      />,
    );
    expect(screen.getByRole('button', { name: /^custom$/i })).toHaveAttribute(
      'aria-pressed',
      'true',
    );
    expect(screen.getByLabelText(/^from$/i)).toBeInTheDocument();

    rerender(<TimeRangePicker value={{ preset: 'all' }} onChange={onChange} now={NOW} />);

    expect(screen.getByRole('button', { name: /^all$/i })).toHaveAttribute('aria-pressed', 'true');
    expect(screen.getByRole('button', { name: /^custom$/i })).toHaveAttribute(
      'aria-pressed',
      'false',
    );
    // The custom editor (and the previous run's bounds) is gone.
    expect(screen.queryByLabelText(/^from$/i)).not.toBeInTheDocument();
  });

  it('does not lose in-progress custom edits to a re-render with an unchanged applied value', () => {
    // The re-sync effect keys on value.preset/from/to, not object
    // identity — a parent re-rendering with an equal-by-value object
    // (e.g. an unrelated sibling state update) must not wipe out text the
    // operator is mid-typing into an unapplied custom range.
    const onChange = vi.fn();
    const { rerender } = render(
      <TimeRangePicker value={{ preset: 'all' }} onChange={onChange} now={NOW} />,
    );
    fireEvent.click(screen.getByRole('button', { name: /^custom$/i }));
    fireEvent.change(screen.getByLabelText(/^from$/i), { target: { value: '2026-08-10T09:30' } });

    rerender(<TimeRangePicker value={{ preset: 'all' }} onChange={onChange} now={NOW} />);

    expect((screen.getByLabelText(/^from$/i) as HTMLInputElement).value).toBe('2026-08-10T09:30');
  });
});

describe('toNetflowRange', () => {
  it('returns undefined for the all preset — the API must see no filter at all', () => {
    expect(toNetflowRange({ preset: 'all' })).toBeUndefined();
  });

  it('returns undefined for a custom preset with no bounds entered yet', () => {
    expect(toNetflowRange({ preset: 'custom' })).toBeUndefined();
  });

  it('carries the from/to through unchanged for a real range', () => {
    expect(toNetflowRange({ preset: '24h', from: '2026-08-16T15:00:00.000Z' })).toEqual({
      from: '2026-08-16T15:00:00.000Z',
      to: undefined,
    });
    expect(
      toNetflowRange({ preset: 'custom', from: 'A', to: 'B' }),
    ).toEqual({ from: 'A', to: 'B' });
  });
});
