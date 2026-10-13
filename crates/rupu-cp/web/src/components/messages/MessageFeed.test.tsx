// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { api, ApiError, type MessageRoute, type MessageStreamFrame, type SpaceMessage } from '../../lib/api';
import { EMPTY_LOG, withMessages } from '../../lib/messages';
import { useMessageSpace, type MessageSpace } from '../../lib/useMessageSpace';
import MessageFeed from './MessageFeed';
import { FeedView } from './FeedView';
import { ALL_KINDS } from '../../lib/messages';
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

  it('filters by kind and by participant', () => {
    render(<MessageFeed route="runs" space={space()} workflow />);
    fireEvent.click(screen.getByTestId('kind-direct'));
    expect(row('msg_01JQ8MA003')).toBeNull();
    expect(row('msg_01JQ8MA001')).not.toBeNull();
    fireEvent.click(screen.getByTestId('kind-direct'));
    expect(row('msg_01JQ8MA003')).not.toBeNull();

    const otter = screen.getAllByTestId('participant').find((b) => b.textContent?.includes('otter'))!;
    fireEvent.click(otter);
    // otter sent msg_009 and was reached by msg_001 / 004 / 005 / 007; not by heron's DM to lynx#1.
    expect(row('msg_01JQ8MA009')).not.toBeNull();
    expect(row('msg_01JQ8MA001')).not.toBeNull();
    expect(row('msg_01JQ8MA003')).toBeNull();
    fireEvent.click(otter);
    expect(row('msg_01JQ8MA003')).not.toBeNull();
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
        filter={{ kinds: ALL_KINDS, involving: null }}
        onToggleKind={() => {}}
        onInvolving={() => {}}
      />,
    );
    const mounted = document.querySelectorAll('[data-msg-id]').length;
    expect(mounted).toBeGreaterThan(0);
    expect(mounted).toBeLessThan(100);
  });
});
