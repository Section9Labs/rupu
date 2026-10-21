// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen, within } from '@testing-library/react';
import MessageListPanel from './MessageListPanel';
import { ThemeProvider } from '../theme/ThemeProvider';
import { buildFlowModel } from '../../lib/flow/flowModel';
import { RUN, fixtureInput } from '../../lib/flow/fixtures';

afterEach(cleanup);

describe('MessageListPanel', () => {
  it('list_tags_rows_by_space_and_selects', () => {
    const model = buildFlowModel(fixtureInput());
    const onSelect = vi.fn();
    const spaces = new Map([
      ['flow', 'Flow'],
      [RUN.gannet1, 'gannet#1'],
    ]);
    render(
      <ThemeProvider>
        <MessageListPanel messages={model.messages} spaces={spaces} selected="msg_005" onSelect={onSelect} />
      </ThemeProvider>,
    );
    const rows = screen.getAllByRole('option');
    expect(rows).toHaveLength(model.messages.length);
    const flowRow = screen.getByTestId('msg-row-msg_005');
    expect(within(flowRow).getByText('Flow')).toBeInTheDocument();
    expect(within(flowRow).getByText('direct')).toBeInTheDocument();
    expect(flowRow).toHaveAttribute('aria-selected', 'true');
    const frameMsg = model.messages.find((m) => m.space === RUN.gannet1)!;
    expect(within(screen.getByTestId(`msg-row-${frameMsg.id}`)).getByText('gannet#1')).toBeInTheDocument();
    fireEvent.click(screen.getByTestId('msg-row-msg_004'));
    expect(onSelect).toHaveBeenCalledWith('msg_004');
    // Clicking the selected row again clears the selection.
    fireEvent.click(flowRow);
    expect(onSelect).toHaveBeenLastCalledWith(null);
  });
});
