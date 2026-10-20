// @vitest-environment jsdom
// useFlowStop's pending state belongs to one flow: moving the hook to another
// flow id starts clean, even when both flows read `running` throughout.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, fireEvent, act } from '@testing-library/react';
import { agentiflowsApi, type StopAnswer } from '../../lib/api3b/agentiflows';
import { useFlowStop } from './useFlowStop';

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

function Controls({ id }: { id: string }) {
  const { buttons, panel } = useFlowStop({ id, running: true, coordinatorGone: false, liveUnits: 2, reload: () => {} });
  return (
    <div>
      {buttons}
      {panel}
    </div>
  );
}

describe('useFlowStop', () => {
  it('a stop requested on one flow is not shown on the next', async () => {
    vi.spyOn(agentiflowsApi, 'stop').mockResolvedValue({ queued: true });
    const { rerender } = render(<Controls id="af_A" />);
    fireEvent.click(screen.getByRole('button', { name: 'Stop' }));
    expect(await screen.findByText('stop requested')).toBeInTheDocument();

    rerender(<Controls id="af_B" />);
    expect(screen.queryByText('stop requested')).not.toBeInTheDocument();
    expect(screen.queryByText(/stops after this round/i)).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Stop' })).toBeEnabled();
  });

  it("a stop answered after the page moved on does not land on the next flow", async () => {
    let answer: (a: StopAnswer) => void = () => {};
    vi.spyOn(agentiflowsApi, 'stop').mockReturnValue(new Promise<StopAnswer>((r) => (answer = r)));
    const { rerender } = render(<Controls id="af_A" />);
    fireEvent.click(screen.getByRole('button', { name: 'Stop' }));
    rerender(<Controls id="af_B" />);
    await act(async () => answer({ queued: true }));
    expect(screen.queryByText('stop requested')).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Stop' })).toBeEnabled();
  });

  it('an open Stop now card does not carry over to the next flow', () => {
    const { rerender } = render(<Controls id="af_A" />);
    fireEvent.click(screen.getByRole('button', { name: 'Stop now' }));
    expect(screen.getByRole('alertdialog', { name: 'Stop now?' })).toBeInTheDocument();
    rerender(<Controls id="af_B" />);
    expect(screen.queryByRole('alertdialog')).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Stop now' })).toBeEnabled();
  });
});
