// @vitest-environment jsdom
// saveBlob — the browser download hand-off shared by the report export dialog
// and the per-finding export buttons.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { REVOKE_DELAY_MS, saveBlob } from './download';

let clicked: HTMLAnchorElement[] = [];
let createObjectURL: ReturnType<typeof vi.fn>;
let revokeObjectURL: ReturnType<typeof vi.fn>;

beforeEach(() => {
  vi.useFakeTimers();
  clicked = [];
  createObjectURL = vi.fn(() => 'blob:mock-url');
  revokeObjectURL = vi.fn();
  Object.defineProperty(URL, 'createObjectURL', { value: createObjectURL, configurable: true, writable: true });
  Object.defineProperty(URL, 'revokeObjectURL', { value: revokeObjectURL, configurable: true, writable: true });
  vi.spyOn(HTMLAnchorElement.prototype, 'click').mockImplementation(function (this: HTMLAnchorElement) {
    clicked.push(this);
  });
});

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

describe('saveBlob', () => {
  it('clicks a hidden download anchor pointing at an object URL, then removes it', () => {
    saveBlob(new Blob(['x']), 'fallback.md');
    expect(createObjectURL).toHaveBeenCalledTimes(1);
    expect(clicked).toHaveLength(1);
    const a = clicked[0];
    expect(a.getAttribute('href')).toBe('blob:mock-url');
    expect(a.download).toBe('fallback.md');
    expect(a.hidden).toBe(true);
    expect(document.body.contains(a)).toBe(false);
  });

  it("prefers the blob's own name (the server's Content-Disposition) over the fallback", () => {
    saveBlob(new File(['x'], 'from-server.pdf'), 'fallback.pdf');
    expect(clicked[0].download).toBe('from-server.pdf');
  });

  it('keeps the object URL alive long enough for a large download to start', () => {
    expect(REVOKE_DELAY_MS).toBeGreaterThanOrEqual(10_000);
    saveBlob(new Blob(['x']), 'r.md');
    vi.advanceTimersByTime(REVOKE_DELAY_MS - 1);
    expect(revokeObjectURL).not.toHaveBeenCalled();
    vi.advanceTimersByTime(1);
    expect(revokeObjectURL).toHaveBeenCalledWith('blob:mock-url');
  });
});
