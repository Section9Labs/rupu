// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { TimeAxis } from './TimeAxis';
import { ThemeProvider } from '../theme/ThemeProvider';

afterEach(cleanup);

const bands = [{ key: 'r1', label: 'Round 1', from: 0, to: 5000 }];
const items = [{ key: 'm1', at: 2000, kind: 'message' as const, sub: 'operator' }];

function mount(t: number | null, live: boolean, cb: Record<string, () => void> & { onScrub?: (t: number) => void }) {
  return render(
    <ThemeProvider>
      <TimeAxis
        bands={bands}
        items={items}
        bounds={{ start: 0, end: 10000 }}
        t={t}
        live={live}
        onScrub={cb.onScrub ?? (() => {})}
        onReplay={cb.onReplay ?? (() => {})}
        onRelease={cb.onRelease ?? (() => {})}
      />
    </ThemeProvider>,
  );
}

describe('TimeAxis', () => {
  it('scrub_drag_reports_times_and_live_pill_shows_when_released', () => {
    const onScrub = vi.fn();
    const onReplay = vi.fn();
    const { container, rerender } = mount(null, true, { onScrub, onReplay });
    expect(screen.getByText('live')).toBeInTheDocument();
    const track = container.querySelector('[data-testid="time-track"]') as HTMLElement;
    track.getBoundingClientRect = () => ({ left: 0, width: 200, top: 0, height: 20, right: 200, bottom: 20, x: 0, y: 0, toJSON: () => ({}) });
    track.setPointerCapture = () => {};
    fireEvent.pointerDown(track, { clientX: 100, pointerId: 1 });
    fireEvent.pointerMove(track, { clientX: 150, pointerId: 1 });
    fireEvent.pointerUp(track, { clientX: 150, pointerId: 1 });
    expect(onScrub).toHaveBeenNthCalledWith(1, 5000);
    expect(onScrub).toHaveBeenNthCalledWith(2, 7500);
    fireEvent.pointerMove(track, { clientX: 20, pointerId: 1 });
    expect(onScrub).toHaveBeenCalledTimes(2);
    fireEvent.click(screen.getByRole('button', { name: /replay/i }));
    expect(onReplay).toHaveBeenCalled();
    // scrubbed: no live pill
    rerender(
      <ThemeProvider>
        <TimeAxis bands={bands} items={items} bounds={{ start: 0, end: 10000 }} t={7500} live onScrub={onScrub} onReplay={onReplay} onRelease={() => {}} />
      </ThemeProvider>,
    );
    expect(screen.queryByText('live')).toBeNull();
  });

  it('keyboard_steps_the_scrubber', () => {
    const onScrub = vi.fn();
    mount(5000, false, { onScrub });
    const slider = screen.getByRole('slider');
    expect(slider).toHaveAttribute('aria-valuenow', '5000');
    fireEvent.keyDown(slider, { key: 'ArrowRight' });
    fireEvent.keyDown(slider, { key: 'ArrowLeft', shiftKey: true });
    fireEvent.keyDown(slider, { key: 'Home' });
    fireEvent.keyDown(slider, { key: 'End' });
    expect(onScrub.mock.calls.map((c) => c[0])).toEqual([5100, 4000, 0, 10000]);
    expect(screen.getByRole('button', { name: 'Show all' })).toBeInTheDocument();
  });

  it('show_all_is_hidden_while_not_scrubbed', () => {
    mount(null, false, {});
    expect(screen.queryByRole('button', { name: 'Show all' })).toBeNull();
  });

  it('a_scrubbed_mount_places_the_scrubber_without_measuring', () => {
    mount(2500, false, {});
    expect(screen.getByTestId('time-scrubber').style.transform).toBe('translateX(25%)');
  });

  it('from_live_only_a_move_back_starts_scrubbing', () => {
    const onScrub = vi.fn();
    mount(null, true, { onScrub });
    const slider = screen.getByRole('slider');
    fireEvent.keyDown(slider, { key: 'ArrowRight' });
    fireEvent.keyDown(slider, { key: 'End' });
    expect(onScrub).not.toHaveBeenCalled();
    fireEvent.keyDown(slider, { key: 'ArrowLeft' });
    fireEvent.keyDown(slider, { key: 'Home' });
    expect(onScrub.mock.calls.map((c) => c[0])).toEqual([9900, 0]);
  });

  it('an_inferred_band_says_its_start_is_inferred', () => {
    render(
      <ThemeProvider>
        <TimeAxis
          bands={[
            { key: 'a', label: 'collect', from: 0, to: 4000, row: 'step' },
            { key: 'b', label: 'report', from: 4000, to: null, row: 'step', inferred: true },
          ]}
          items={[]}
          bounds={{ start: 0, end: 10000 }}
          t={null}
          live
          onScrub={() => {}}
          onReplay={() => {}}
          onRelease={() => {}}
        />
      </ThemeProvider>,
    );
    const exact = screen.getByTitle('collect');
    expect(exact).not.toHaveAttribute('data-inferred');
    const inferred = screen.getByTitle('report · start inferred');
    expect(inferred).toHaveAttribute('data-inferred', 'true');
  });

  it('with_onPick_a_tap_names_what_it_hit_and_a_drag_scrubs', () => {
    const onScrub = vi.fn();
    const onPick = vi.fn();
    render(
      <ThemeProvider>
        <TimeAxis bands={bands} items={items} bounds={{ start: 0, end: 10000 }} t={null} live={false}
          onScrub={onScrub} onReplay={() => {}} onRelease={() => {}} onPick={onPick} />
      </ThemeProvider>,
    );
    const track = screen.getByTestId('time-track');
    track.getBoundingClientRect = () => ({ left: 0, width: 1000, top: 0, height: 20, right: 1000, bottom: 20, x: 0, y: 0, toJSON: () => ({}) });
    // A tap on the band: no scrub, the band.
    fireEvent.pointerDown(track, { clientX: 400, pointerId: 1 });
    fireEvent.pointerUp(track, { clientX: 401, pointerId: 1 });
    expect(onPick).toHaveBeenLastCalledWith({ kind: 'band', key: 'r1' });
    // On the tick (at 200px): the message.
    fireEvent.pointerDown(track, { clientX: 202, pointerId: 1 });
    fireEvent.pointerUp(track, { clientX: 202, pointerId: 1 });
    expect(onPick).toHaveBeenLastCalledWith({ kind: 'message', key: 'm1' });
    expect(onScrub).not.toHaveBeenCalled();
    // A drag scrubs and names nothing.
    fireEvent.pointerDown(track, { clientX: 100, pointerId: 1 });
    fireEvent.pointerMove(track, { clientX: 150, pointerId: 1 });
    fireEvent.pointerUp(track, { clientX: 150, pointerId: 1 });
    expect(onScrub).toHaveBeenLastCalledWith(1500);
    expect(onPick).toHaveBeenCalledTimes(2);
  });
});
