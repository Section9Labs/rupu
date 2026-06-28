// @vitest-environment jsdom
// HostStatusBadge — asserts the right semantic token class is applied per status.

import '@testing-library/jest-dom/vitest';
import { describe, it, expect } from 'vitest';
import { render } from '@testing-library/react';
import { HostStatusBadge } from './HostStatusBadge';

describe('HostStatusBadge', () => {
  it('online → ok tokens', () => {
    const { container } = render(<HostStatusBadge status="online" />);
    const chip = container.firstElementChild as HTMLElement;
    expect(chip.className).toMatch(/\bok\b/);
    expect(chip.textContent).toBe('online');
  });

  it('stale → warn tokens', () => {
    const { container } = render(<HostStatusBadge status="stale" />);
    const chip = container.firstElementChild as HTMLElement;
    expect(chip.className).toMatch(/\bwarn\b/);
    expect(chip.textContent).toBe('stale');
  });

  it('offline → err tokens', () => {
    const { container } = render(<HostStatusBadge status="offline" />);
    const chip = container.firstElementChild as HTMLElement;
    expect(chip.className).toMatch(/\berr\b/);
    expect(chip.textContent).toBe('offline');
  });
});
