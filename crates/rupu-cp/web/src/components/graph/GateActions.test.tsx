// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import GateActions, { type GateControl, type GateDecision } from './GateActions';

afterEach(cleanup);

function control(state: Partial<GateDecision> = {}) {
  const c = {
    state: vi.fn(() => ({ pending: false, decision: null, error: null, ...state })),
    approve: vi.fn(),
    reject: vi.fn(),
  } satisfies GateControl;
  return c;
}

describe('GateActions', () => {
  it('approves the gate it sits on', () => {
    const c = control();
    render(<GateActions stepId="ship_gate" control={c} />);
    fireEvent.click(screen.getByRole('button', { name: 'Approve ship_gate' }));
    expect(c.approve).toHaveBeenCalledWith('ship_gate');
    expect(c.reject).not.toHaveBeenCalled();
  });

  it('asks once more before rejecting, and Keep backs out', () => {
    const c = control();
    render(<GateActions stepId="ship_gate" control={c} />);
    fireEvent.click(screen.getByRole('button', { name: 'Reject ship_gate' }));
    expect(c.reject).not.toHaveBeenCalled();
    expect(screen.getByText('Reject?')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Keep' }));
    expect(screen.getByRole('button', { name: 'Approve ship_gate' })).toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: 'Reject ship_gate' }));
    fireEvent.click(screen.getByRole('button', { name: 'Confirm rejection for ship_gate' }));
    expect(c.reject).toHaveBeenCalledWith('ship_gate');
  });

  it('holds both buttons while a request is in flight', () => {
    render(<GateActions stepId="g" control={control({ pending: true })} />);
    expect(screen.getByRole('button', { name: 'Approve g' })).toBeDisabled();
    expect(screen.getByRole('button', { name: 'Approve g' })).toHaveTextContent('Approving…');
    expect(screen.getByRole('button', { name: 'Reject g' })).toBeDisabled();
  });

  it('says "resuming" once approved, until the run moves the gate on', () => {
    render(<GateActions stepId="g" control={control({ decision: 'approved' })} />);
    expect(screen.getByTestId('gate-resolved')).toHaveTextContent('approved · resuming…');
    expect(screen.queryByRole('button')).toBeNull();
  });

  it('shows a failed request beside the buttons', () => {
    render(<GateActions stepId="g" control={control({ error: 'run is not awaiting approval' })} />);
    expect(screen.getByRole('alert')).toHaveTextContent('run is not awaiting approval');
    expect(screen.getByRole('button', { name: 'Approve g' })).toBeEnabled();
  });
});
