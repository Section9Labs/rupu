// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { QueryHelp } from './QueryHelp';
import { FINDING_FIELDS } from '../../lib/findingQuery/fields';
import { SKILL_FIELDS } from '../../lib/query/lists/skills';
import { clearQueryFieldsCache } from '../../lib/api3b/library';

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  clearQueryFieldsCache();
});

const jsonResponse = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status });

describe('QueryHelp', () => {
  it('fetches nothing until it opens, then lists the server fields', async () => {
    const fetchMock = vi.spyOn(globalThis, 'fetch').mockImplementation(async () =>
      jsonResponse({
        list: 'findings',
        fields: [{ key: 'severity', aliases: ['sev'], kind: 'severity', values: ['low', 'high'] }],
      }),
    );
    render(<QueryHelp fields={FINDING_FIELDS} label="Filter findings" />);
    expect(fetchMock).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole('button', { name: 'Query fields' }));

    const pop = await screen.findByRole('dialog', { name: 'Findings query fields' });
    expect(fetchMock.mock.calls[0][0]).toBe('/api/query/findings/fields');
    expect(await screen.findByText('severity')).toBeInTheDocument();
    expect(pop).toHaveTextContent('ordered · supports >= <=');
    expect(pop).toHaveTextContent('low high');
    expect(pop).toHaveTextContent('Bare words search the text.');
    // Only the server's fields: a local field it doesn't list is not offered.
    expect(pop).not.toHaveTextContent('owner');
  });

  it('uses the local registry for a list the browser filters', () => {
    const fetchMock = vi.spyOn(globalThis, 'fetch');
    render(<QueryHelp fields={SKILL_FIELDS} label="Filter skills" />);

    fireEvent.click(screen.getByRole('button', { name: 'Query fields' }));

    expect(screen.getByRole('dialog', { name: 'Skills query fields' })).toHaveTextContent(SKILL_FIELDS[0].key);
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it('falls back to the local registry, saying so, when the server can not answer', async () => {
    vi.spyOn(globalThis, 'fetch').mockImplementation(async () => jsonResponse({ error: 'down' }, 502));
    render(<QueryHelp fields={FINDING_FIELDS} label="Filter findings" />);

    fireEvent.click(screen.getByRole('button', { name: 'Query fields' }));

    expect(await screen.findByText(/couldn't read the server's fields/)).toBeInTheDocument();
    expect(screen.getByRole('dialog')).toHaveTextContent('owner');
  });

  it('closes on Escape and gives focus back to the button', () => {
    render(<QueryHelp fields={SKILL_FIELDS} label="Filter skills" />);
    const button = screen.getByRole('button', { name: 'Query fields' });
    fireEvent.click(button);

    fireEvent.keyDown(document, { key: 'Escape' });

    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    expect(button).toHaveFocus();
    expect(button).toHaveAttribute('aria-expanded', 'false');
  });
});
