// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import FlowToolbar from './FlowToolbar';

afterEach(cleanup);

describe('FlowToolbar', () => {
  it('toggles_kinds_and_picks_a_participant', () => {
    const onKinds = vi.fn();
    const onParticipant = vi.fn();
    render(
      <FlowToolbar
        kinds={new Set(['post', 'direct'])}
        participant={null}
        participants={['lead', 'lynx#1']}
        onKinds={onKinds}
        onParticipant={onParticipant}
      />,
    );
    expect(screen.getByRole('button', { name: 'Direct' })).toHaveAttribute('aria-pressed', 'true');
    expect(screen.getByRole('button', { name: 'Broadcast' })).toHaveAttribute('aria-pressed', 'false');
    fireEvent.click(screen.getByRole('button', { name: 'Broadcast' }));
    expect(onKinds).toHaveBeenCalledWith(new Set(['post', 'direct', 'broadcast']));
    fireEvent.click(screen.getByRole('button', { name: 'Direct' }));
    expect(onKinds).toHaveBeenLastCalledWith(new Set(['post']));
    fireEvent.change(screen.getByLabelText('Participant'), { target: { value: 'lynx#1' } });
    expect(onParticipant).toHaveBeenCalledWith('lynx#1');
    fireEvent.change(screen.getByLabelText('Participant'), { target: { value: '' } });
    expect(onParticipant).toHaveBeenLastCalledWith(null);
  });
});
