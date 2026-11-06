// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import CollapsiblePanel, { PANEL_KEY } from './CollapsiblePanel';
import { boardCounts } from './BoardRail';

let store: Map<string, string>;
beforeEach(() => {
  store = new Map();
  vi.stubGlobal('localStorage', {
    getItem: (k: string) => store.get(k) ?? null,
    setItem: (k: string, v: string) => void store.set(k, v),
    removeItem: (k: string) => void store.delete(k),
  });
});
afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

const panel = () => render(
  <CollapsiblePanel name="run-messages" title="Messages" count={15}>
    <p>the body</p>
  </CollapsiblePanel>,
);

describe('CollapsiblePanel', () => {
  it('the_title_row_folds_and_unfolds_and_is_remembered', () => {
    panel();
    const toggle = screen.getByRole('button', { name: /Messages/ });
    expect(toggle).toHaveAttribute('aria-expanded', 'true');
    expect(screen.getByText('the body')).toBeVisible();
    fireEvent.click(toggle);
    // Folded: the title and its count stay, the body hides.
    expect(toggle).toHaveAttribute('aria-expanded', 'false');
    expect(toggle).toHaveTextContent('Messages15');
    expect(screen.getByText('the body')).not.toBeVisible();
    expect(window.localStorage.getItem(PANEL_KEY('run-messages'))).toBe('collapsed');
    // The keyboard reaches it: a real button (Enter and Space click it).
    expect(toggle.tagName).toBe('BUTTON');
    expect(toggle).toHaveAttribute('type', 'button');
    fireEvent.click(toggle);
    expect(toggle).toHaveAttribute('aria-expanded', 'true');
    expect(window.localStorage.getItem(PANEL_KEY('run-messages'))).toBe('expanded');
    fireEvent.click(toggle);
    expect(toggle).toHaveAttribute('aria-expanded', 'false');
    // A new page reads it back folded.
    cleanup();
    panel();
    expect(screen.getByRole('button', { name: /Messages/ })).toHaveAttribute('aria-expanded', 'false');
  });

  it('without_storage_it_opens_expanded_and_still_folds', () => {
    const broken = {
      getItem: () => {
        throw new Error('blocked');
      },
      setItem: () => {
        throw new Error('blocked');
      },
    };
    vi.stubGlobal('localStorage', broken);
    panel();
    const toggle = screen.getByRole('button', { name: /Messages/ });
    expect(toggle).toHaveAttribute('aria-expanded', 'true');
    fireEvent.click(toggle);
    expect(toggle).toHaveAttribute('aria-expanded', 'false');
  });

  it('a_folded_board_says_what_it_holds', () => {
    expect(boardCounts(null, [])).toBe('none');
    const board = { claims: [{ key: 'k', owner: 'a', acquired_at: '', lease_expires_at: null, ended_at: null, state: 'released' as const, key_known: true }], directives: [], journal: true };
    const post = { id: 'p' } as Parameters<typeof boardCounts>[1][number];
    expect(boardCounts(board, Array(7).fill(post))).toBe('1 claim · 7 posts');
    expect(boardCounts({ kind: 'unavailable', reason: 'x' }, [post])).toBe('1 post');
  });
});
