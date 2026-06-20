// @vitest-environment jsdom
import { describe, it, expect, vi } from 'vitest';
import '@testing-library/jest-dom/vitest';
import { render } from '@testing-library/react';
import { useInfiniteScroll } from './useInfiniteScroll';

function Harness({ hasMore, loadMore }: { hasMore: boolean; loadMore: () => void }) {
  const { sentinelRef } = useInfiniteScroll({ hasMore, loadMore });
  return <div ref={sentinelRef}>sentinel</div>;
}

describe('useInfiniteScroll', () => {
  it('mounts and renders the sentinel without crashing', () => {
    const loadMore = vi.fn();
    const { getByText } = render(<Harness hasMore={false} loadMore={loadMore} />);
    expect(getByText('sentinel')).toBeInTheDocument();
  });

  it('does not call loadMore when hasMore is false', async () => {
    const loadMore = vi.fn();
    render(<Harness hasMore={false} loadMore={loadMore} />);
    await new Promise((r) => setTimeout(r, 20));
    expect(loadMore).not.toHaveBeenCalled();
  });
});
