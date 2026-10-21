// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import BoardRail from './BoardRail';
import { ThemeProvider } from '../theme/ThemeProvider';
import { buildFlowModel } from '../../lib/flow/flowModel';
import { FLOW_BOARD, fixtureInput } from '../../lib/flow/fixtures';

afterEach(cleanup);

const wrap = (ui: React.ReactElement) => render(<ThemeProvider>{ui}</ThemeProvider>);
const model = buildFlowModel(fixtureInput());

describe('BoardRail', () => {
  it('board_unavailable_is_said_in_place', () => {
    // The reason is the sentence (`boardUnavailable` writes it).
    wrap(<BoardRail board={{ kind: 'unavailable', reason: 'This host can\'t show the board: too old.' }} posts={[]} onSelect={() => {}} />);
    expect(screen.getByText("This host can't show the board: too old.")).toBeInTheDocument();
    expect(screen.queryByText('Claims')).toBeNull();
    cleanup();
    wrap(<BoardRail board={{ kind: 'unavailable', reason: '' }} posts={[]} onSelect={() => {}} />);
    expect(screen.getByText("This host can't show the board.")).toBeInTheDocument();
  });

  it('an_unavailable_board_still_lists_its_posts', () => {
    wrap(<BoardRail board={{ kind: 'unavailable', reason: 'host too old' }} posts={model.board.posts} onSelect={() => {}} />);
    expect(screen.getByText('host too old')).toBeInTheDocument();
    expect(screen.getByText(/token-fixation advisory/)).toBeInTheDocument();
    expect(screen.queryByText('Claims')).toBeNull();
  });

  it('pre_journal_board_says_so', () => {
    const board = {
      claims: [{ ...FLOW_BOARD.claims[2], key: 'file-stem', key_known: false }],
      directives: [],
      journal: false,
    };
    wrap(<BoardRail board={board} posts={[]} onSelect={() => {}} />);
    expect(screen.getByText(/claims before this version show only while held/)).toBeInTheDocument();
    expect(screen.getByText('claim (key not recorded)')).toBeInTheDocument();
  });

  it('lists_claims_posts_and_directives_and_selects_a_post', () => {
    const onSelect = vi.fn();
    const withRetraction = {
      ...FLOW_BOARD,
      directives: [
        { ...FLOW_BOARD.directives[0], retracted: true, retracted_at: '2026-10-20T15:00:00Z' },
        FLOW_BOARD.directives[1],
      ],
    };
    wrap(<BoardRail board={withRetraction} posts={model.board.posts} onSelect={onSelect} />);
    expect(screen.getByText('route map')).toBeInTheDocument();
    expect(screen.getByText('engagement report')).toBeInTheDocument();
    expect(screen.getByText('held')).toBeInTheDocument();
    expect(screen.getAllByText('released')).toHaveLength(2);
    expect(screen.getByText(/retracted/)).toBeInTheDocument();
    fireEvent.click(screen.getByText(/token-fixation advisory/));
    expect(onSelect).toHaveBeenCalledWith('msg_003');
  });
});
