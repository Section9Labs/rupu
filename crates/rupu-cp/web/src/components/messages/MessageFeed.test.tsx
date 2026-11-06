// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { api, ApiError, type MessageRoute, type MessageStreamFrame, type SpaceMessage } from '../../lib/api';
import { EMPTY_LOG, withMessages } from '../../lib/messages';
import { useMessageSpace, type MessageSpace } from '../../lib/useMessageSpace';
import MessageFeed from './MessageFeed';
import { FeedView } from './FeedView';
import { MESSAGES, PARTICIPANTS, RUN_ID } from './fixtures';

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

function space(over: Partial<MessageSpace> = {}): MessageSpace {
  return {
    status: 'ready',
    reason: null,
    space: 'present',
    legacy: false,
    participants: PARTICIPANTS,
    log: withMessages(EMPTY_LOG, MESSAGES),
    loaded: true,
    caughtUp: true,
    streaming: true,
    refreshable: false,
    refresh: () => {},
    pending: [],
    sendError: null,
    blocked: null,
    send: async () => true,
    unread: 0,
    ...over,
  };
}

const row = (id: string) => document.getElementById(`msg-${id}`) as HTMLElement;
const shown = () => [...document.querySelectorAll<HTMLElement>('[data-msg-id]')].map((e) => e.dataset.msgId);
const chips = () => screen.queryAllByRole('button', { name: /^Remove / }).map((b) => b.getAttribute('aria-label')!.slice('Remove '.length));

describe('MessageFeed renders every kind', () => {
  it('gives each log kind its own treatment', () => {
    render(<MessageFeed route="runs" space={space()} workflow />);
    expect(row('msg_01JQ8MA001').dataset.kind).toBe('post');
    expect(row('msg_01JQ8MA004').dataset.kind).toBe('mention');
    expect(within(row('msg_01JQ8MA004')).getByText('@heron')).toBeInTheDocument();
    expect(row('msg_01JQ8MA003').dataset.kind).toBe('direct');
    expect(within(row('msg_01JQ8MA003')).getByText('direct')).toBeInTheDocument();
    expect(within(row('msg_01JQ8MA005')).getByText('broadcast')).toBeInTheDocument();
    expect(within(row('msg_01JQ8MA007')).getByText('urgent')).toBeInTheDocument();
    expect(within(row('msg_01JQ8MA007')).getByText('via control plane')).toBeInTheDocument();
    expect(within(row('msg_01JQ8MA002')).getByText('posted a directive')).toBeInTheDocument();
    expect(within(row('msg_01JQ8MA008')).getByText('retracted a directive')).toBeInTheDocument();
    expect(within(row('msg_01JQ8MA006')).getByTestId('undelivered')).toHaveTextContent('undelivered: no live participant for role:crawler');
    expect(within(row('msg_01JQ8MA007')).getByTestId('receipts')).toHaveTextContent('seen by 3');
    expect(within(row('msg_01JQ8MA003')).getByTestId('receipts')).toHaveTextContent('seen by lynx#1 · turn 4');
    // Only the "broadcast" kind is labelled broadcast: a channel post is not.
    expect(within(row('msg_01JQ8MA001')).queryByText('broadcast')).toBeNull();
  });

  it('pins the directive, folded with its retraction', () => {
    render(<MessageFeed route="runs" space={space()} workflow />);
    const panel = screen.getByTestId('directives-panel');
    expect(panel).toHaveTextContent('0 standing');
    fireEvent.click(within(panel).getByText('1 retracted'));
    expect(within(panel).getByTestId('directive')).toHaveTextContent('retracted by heron');
  });

  it('a kind pill selects only its kinds, and clicking it again clears it', () => {
    render(<MessageFeed route="runs" space={space()} workflow />);
    const pill = screen.getByTestId('kind-direct');
    expect(pill).toHaveTextContent('Direct 2');
    fireEvent.click(pill);
    expect(pill).toHaveAttribute('aria-pressed', 'true');
    expect(shown()).toEqual(['msg_01JQ8MA003', 'msg_01JQ8MA006']);
    expect(chips()).toEqual(['kind: direct']);

    // Directives covers a directive and its retraction; it replaces the kind term.
    fireEvent.click(screen.getByTestId('kind-directive'));
    expect(pill).toHaveAttribute('aria-pressed', 'false');
    expect(shown()).toEqual(['msg_01JQ8MA002', 'msg_01JQ8MA008']);

    fireEvent.click(screen.getByTestId('kind-directive'));
    expect(shown()).toHaveLength(MESSAGES.length);
    expect(chips()).toEqual([]);
  });

  it('a participant chip filters to messages involving it; clicking again clears', () => {
    render(<MessageFeed route="runs" space={space()} workflow />);
    const otter = screen.getAllByTestId('participant').find((b) => b.textContent?.includes('otter'))!;
    fireEvent.click(otter);
    expect(otter).toHaveAttribute('aria-pressed', 'true');
    expect(chips()).toEqual(['involving: otter']);
    // otter sent 009 and was reached by 001, 002, 004, 005, 007, 008; not by heron's DM to lynx#1 (003) or lynx#1's to role:crawler (006).
    expect(shown()).toEqual(['msg_01JQ8MA001', 'msg_01JQ8MA002', 'msg_01JQ8MA004', 'msg_01JQ8MA005', 'msg_01JQ8MA007', 'msg_01JQ8MA008', 'msg_01JQ8MA009']);
    fireEvent.click(otter);
    expect(otter).toHaveAttribute('aria-pressed', 'false');
    expect(shown()).toHaveLength(MESSAGES.length);
  });

  it('typing a query filters, and a pill joins it', () => {
    render(<MessageFeed route="runs" space={space()} workflow />);
    const input = screen.getByLabelText('Filter messages');
    fireEvent.change(input, { target: { value: 'from:lynx' } });
    fireEvent.keyDown(input, { key: 'Enter' });
    expect(chips()).toEqual(['from: lynx']);
    expect(shown()).toEqual(['msg_01JQ8MA004', 'msg_01JQ8MA005', 'msg_01JQ8MA006']);

    fireEvent.click(screen.getByTestId('kind-direct'));
    expect(shown()).toEqual(['msg_01JQ8MA006']);

    fireEvent.change(input, { target: { value: 'gateway' } });
    fireEvent.keyDown(input, { key: 'Enter' });
    expect(screen.getByTestId('feed-rows')).toHaveTextContent('No messages match the filter.');
  });

  it('notes a pre-log agentiflow', () => {
    render(<MessageFeed route="agentiflows" space={space({ legacy: true })} />);
    expect(screen.getByTestId('legacy-note')).toHaveTextContent('board posts and directives only');
  });

  it('says when the host cannot serve messages', () => {
    render(<MessageFeed route="runs" space={space({ status: 'unavailable', reason: 'host t1: tunnel hosts do not carry messages' })} />);
    expect(screen.getByText('Messages unavailable')).toBeInTheDocument();
    expect(screen.getByText('host t1: tunnel hosts do not carry messages')).toBeInTheDocument();
  });
});

describe('RecipientPicker', () => {
  function openPicker() {
    fireEvent.click(screen.getByRole('button', { name: /^to/ }));
    return screen.getByRole('listbox');
  }

  it('groups everyone, participants, roles and steps, with the left greyed', () => {
    render(<MessageFeed route="runs" space={space()} workflow />);
    const list = openPicker();
    const groups = within(list).getAllByRole('group').map((g) => g.getAttribute('aria-label'));
    expect(groups).toEqual(['Everyone', 'Participants', 'Roles', 'Steps']);
    const people = within(within(list).getByRole('group', { name: 'Participants' })).getAllByRole('option');
    expect(people.map((o) => [o.textContent?.split(/(?=recon|service|exploit)/)[0], o.dataset.dim ?? null])).toEqual([
      ['heron', null],
      ['lynx#1', null],
      ['otter', 'true'],
    ]);
    const roles = within(within(list).getByRole('group', { name: 'Roles' })).getAllByRole('option');
    expect(roles.map((o) => o.textContent)).toEqual(['role:exploit-verifier0 live', 'role:recon1 live', 'role:service-analyst1 live']);
  });

  it('defaults to everyone on a run and the lead on a flow', () => {
    const { unmount } = render(<MessageFeed route="runs" space={space()} workflow />);
    expect(screen.getByRole('button', { name: /^to/ })).toHaveTextContent('@everyone');
    unmount();
    render(<MessageFeed route="agentiflows" space={space()} />);
    expect(screen.getByRole('button', { name: /^to/ })).toHaveTextContent('@lead');
    expect(within(openPicker()).queryByRole('group', { name: 'Steps' })).toBeNull();
  });
});

/** The real hook behind the feed, over a mocked API. */
function Harness({ route = 'runs' as MessageRoute, live = true }) {
  const s = useMessageSpace({ route, id: RUN_ID }, { live, load: true, reading: true });
  return <MessageFeed route={route} space={s} workflow />;
}

function mockApi(send: () => Promise<unknown>) {
  let onFrame: (f: MessageStreamFrame) => void = () => {};
  let onOpen: () => void = () => {};
  vi.spyOn(api, 'getMessages').mockResolvedValue({
    kind: 'ok',
    view: { participants: PARTICIPANTS, messages: MESSAGES.slice(0, 1), legacy: false, space: 'present', next: null },
  });
  vi.spyOn(api, 'subscribeMessages').mockImplementation((_r, _id, h) => {
    onFrame = h.onFrame;
    onOpen = h.onOpen ?? (() => {});
    return () => {};
  });
  const sendSpy = vi.spyOn(api, 'sendMessage').mockImplementation(send as typeof api.sendMessage);
  return { frame: (f: MessageStreamFrame) => onFrame(f), open: () => onOpen(), sendSpy };
}

/** Render the harness and connect its live stream. */
async function connect(m: ReturnType<typeof mockApi>) {
  render(<Harness />);
  await waitFor(() => expect(api.subscribeMessages).toHaveBeenCalled());
  act(() => m.open());
  await screen.findByTestId('composer');
}

async function type(text: string) {
  fireEvent.change(screen.getByLabelText('Message'), { target: { value: text } });
  await act(async () => {
    fireEvent.click(screen.getByRole('button', { name: 'Send' }));
  });
}

describe('Composer', () => {
  it('is disabled with the reason when the server answers 409', async () => {
    await connect(mockApi(() => Promise.reject(new ApiError(409, 'not running', JSON.stringify({ error: 'workflow run `run_x` is not running (completed)' })))));
    await type('anyone there?');
    expect(screen.getByTestId('composer-disabled')).toHaveTextContent('workflow run `run_x` is not running (completed)');
    expect(screen.getByLabelText('Message')).toBeDisabled();
    expect(screen.queryByTestId('pending')).toBeNull();
  });

  it('shows a send as pending until the stream delivers its line, never before', async () => {
    const api_ = mockApi(() => Promise.resolve({ kind: 'sent', reply: { id: 'msg_NEW', delivery: { resolved: ['heron'] } } }));
    await connect(api_);
    await type('focus on the admin panel');
    expect(api_.sendSpy).toHaveBeenCalledWith('runs', RUN_ID, { to: 'broadcast', body: 'focus on the admin panel', urgent: false }, { host: undefined });
    expect(screen.getByTestId('pending')).toHaveTextContent('waiting for the log');
    expect(row('msg_NEW')).toBeNull();

    const line: SpaceMessage = {
      id: 'msg_NEW',
      ts: '2026-10-13T15:00:00Z',
      kind: 'operator',
      from: 'operator',
      to: 'broadcast',
      body: 'focus on the admin panel',
      via: 'cp',
      delivery: { resolved: ['heron'] },
      seen: [],
    };
    act(() => api_.frame({ t: 'msg', ...line }));
    await waitFor(() => expect(row('msg_NEW')).not.toBeNull());
    expect(screen.queryByTestId('pending')).toBeNull();
  });
});

describe('windowing', () => {
  it('mounts fewer than 100 rows of a 5,000-line log', () => {
    const many: SpaceMessage[] = Array.from({ length: 5000 }, (_, i) => ({
      id: `msg_${String(i).padStart(5, '0')}`,
      ts: new Date(Date.UTC(2026, 9, 13, 10, 0, i)).toISOString(),
      kind: 'post',
      from: 'heron',
      to: null,
      body: `line ${i}`,
      delivery: { resolved: [] },
      seen: [],
    }));
    render(
      <FeedView
        participants={PARTICIPANTS}
        messages={many}
        legacy={false}
        query=""
        onQuery={() => {}}
      />,
    );
    const mounted = document.querySelectorAll('[data-msg-id]').length;
    expect(mounted).toBeGreaterThan(0);
    expect(mounted).toBeLessThan(100);
  });
});
