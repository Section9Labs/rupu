// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { SkillActions } from './SkillActions';

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

const jsonResponse = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status });

function server(answer: (url: string, body: Record<string, unknown>) => Response) {
  return vi.spyOn(globalThis, 'fetch').mockImplementation(async (input, init) =>
    answer(String(input), JSON.parse((init as RequestInit).body as string) as Record<string, unknown>),
  );
}

const sent = (m: ReturnType<typeof vi.spyOn>, n = 0) => JSON.parse((m.mock.calls[n][1] as RequestInit).body as string);

describe('SkillActions', () => {
  it('updates the skill, then asks its owner to read it again', async () => {
    const fetchMock = server(() =>
      jsonResponse({ layer: 'project', results: [{ name: 'nmap', outcome: 'updated', from: 'c1aaaa', to: 'c2bbbb' }] }),
    );
    const onChanged = vi.fn();
    render(<SkillActions name="nmap" layer="project" wsId="ws_1" hasSource onChanged={onChanged} />);

    fireEvent.click(screen.getByRole('button', { name: 'Update' }));

    expect(screen.getByRole('button', { name: 'Updating…' })).toBeDisabled();
    expect(await screen.findByRole('status')).toHaveTextContent('Updated from c1aaaa to c2bbbb.');
    expect(fetchMock.mock.calls[0][0]).toBe('/api/skills/update');
    expect(sent(fetchMock)).toEqual({ names: ['nmap'], project: 'ws_1' });
    expect(onChanged).toHaveBeenCalledTimes(1);
  });

  it('says so when the skill is up to date, without re-reading it', async () => {
    server(() => jsonResponse({ layer: 'global', results: [{ name: 'nmap', outcome: 'up_to_date' }] }));
    const onChanged = vi.fn();
    render(<SkillActions name="nmap" layer="global" hasSource onChanged={onChanged} />);

    fireEvent.click(screen.getByRole('button', { name: 'Update' }));

    expect(await screen.findByRole('status')).toHaveTextContent('Already up to date.');
    expect(onChanged).not.toHaveBeenCalled();
  });

  it('offers to replace an edited copy, confirmed, with force', async () => {
    const fetchMock = server((_u, body) =>
      jsonResponse({
        layer: 'global',
        results: [{ name: 'nmap', ...(body.force ? { outcome: 'updated', from: 'a', to: 'b' } : { outcome: 'locally_modified' }) }],
      }),
    );
    render(<SkillActions name="nmap" layer="global" hasSource onChanged={() => {}} />);
    fireEvent.click(screen.getByRole('button', { name: 'Update' }));
    expect(await screen.findByText(/was edited where it is installed/)).toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: 'Replace local edits…' }));
    expect(screen.getByText(/The edits are lost/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Replace' }));

    await waitFor(() => expect(fetchMock).toHaveBeenCalledTimes(2));
    expect(sent(fetchMock, 1)).toEqual({ names: ['nmap'], force: true });
    expect(await screen.findByText('Updated from a to b.')).toBeInTheDocument();
  });

  it('disables Update for a skill with no source record', () => {
    render(<SkillActions name="nmap" layer="global" hasSource={false} onChanged={() => {}} />);
    expect(screen.getByRole('button', { name: 'Update' })).toBeDisabled();
  });

  it('validates the skill and shows the result inline', async () => {
    const fetchMock = server(() =>
      jsonResponse({ ok: true, skills: [{ name: 'nmap', layer: 'global', path: '/g/nmap', status: 'ok', warnings: ['description is long'] }] }),
    );
    render(<SkillActions name="nmap" layer="global" hasSource onChanged={() => {}} />);

    fireEvent.click(screen.getByRole('button', { name: 'Validate' }));

    const line = await screen.findByRole('status');
    expect(line).toHaveTextContent('Valid');
    expect(line).toHaveTextContent('description is long');
    expect(fetchMock.mock.calls[0][0]).toBe('/api/skills/validate');
    expect(sent(fetchMock)).toEqual({ name: 'nmap', layer: 'global' });
  });

  it('shows an invalid skill and a refused call as alerts', async () => {
    server((url) =>
      url.endsWith('validate')
        ? jsonResponse({ ok: false, skills: [{ name: 'nmap', path: '/g', status: 'invalid', error: 'missing description', warnings: [] }] })
        : jsonResponse({ error: 'updating skills requires `rupu cp serve`' }, 501),
    );
    render(<SkillActions name="nmap" layer="global" hasSource onChanged={() => {}} />);

    fireEvent.click(screen.getByRole('button', { name: 'Validate' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('Invalid: missing description');

    fireEvent.click(screen.getByRole('button', { name: 'Update' }));
    await waitFor(() => expect(screen.getByRole('alert')).toHaveTextContent('requires `rupu cp serve`'));
  });
});
