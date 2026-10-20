// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { afterEach, describe, expect, it } from 'vitest';
import { cleanup, render, screen } from '@testing-library/react';
import { StatusChip } from './StatusChip';

afterEach(cleanup);

describe('StatusChip', () => {
  it('is a full pill with tint + text + ring', () => {
    render(<StatusChip tone="failed">failed</StatusChip>);
    const chip = screen.getByText('failed');
    expect(chip.className).toMatch(/rounded-full/);
    expect(chip.className).toMatch(/\bring-1\b/);
    // Reuses the canonical status pill palette.
    expect(chip.className).toMatch(/text-status-failed/);
  });

  it('shows a dot only when asked', () => {
    const { rerender } = render(<StatusChip tone="running">running</StatusChip>);
    expect(screen.getByText('running').querySelector('span[aria-hidden]')).toBeNull();
    rerender(
      <StatusChip tone="running" dot>
        running
      </StatusChip>,
    );
    const dot = screen.getByText('running').querySelector('span[aria-hidden]');
    expect(dot).not.toBeNull();
    expect(dot?.className).toMatch(/bg-status-running/);
  });

  it('normalizes the done alias to the completed palette', () => {
    render(<StatusChip tone="done">done</StatusChip>);
    expect(screen.getByText('done').className).toMatch(/text-status-done/);
  });

  it('severity tones reuse the sev ramp', () => {
    render(<StatusChip tone="high">high</StatusChip>);
    expect(screen.getByText('high').className).toMatch(/text-sev-high/);
  });

  it('origin tones match the shipped provenance convention (global=info, project=ok)', () => {
    const { rerender } = render(<StatusChip tone="origin-global">global</StatusChip>);
    expect(screen.getByText('global').className).toMatch(/text-info/);
    rerender(<StatusChip tone="origin-project">project</StatusChip>);
    expect(screen.getByText('project').className).toMatch(/text-ok/);
  });

  it('neutral tone is the quiet surface pill', () => {
    render(<StatusChip tone="neutral">paused</StatusChip>);
    expect(screen.getByText('paused').className).toMatch(/bg-surface/);
  });
});
