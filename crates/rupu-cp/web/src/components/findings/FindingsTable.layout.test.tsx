// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { afterEach, describe, expect, it } from 'vitest';
import { cleanup, render, screen } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { FindingsTable } from './FindingsTable';
import type { FindingOut } from '../../lib/api';

afterEach(cleanup);

const LONG_PATH = 'src/some/very/deeply/nested/directory/structure/with/a/long/file_name.rs';

function row(): FindingOut {
  return {
    id: 'fnd_a',
    scope: 'line',
    summary: 'SQL injection in the order lookup',
    severity: 'high',
    file_path: LONG_PATH,
    line_range: [10, 12],
    concern_id: 'a-very-long-concern-identifier-for-the-ledger',
    evidence: { rationale: 'r', references: [] },
    declared_by: { run_id: 'r', model: 'm', surface: 'workflow' },
    declared_at: '2026-10-06T00:00:00Z',
    ws_id: 'ws1',
    project: 'shop-web',
    target_id: 'target-with-a-long-name-0123456789',
    codename: 'jade-reef/heron#1',
    codename_derived: false,
  } as unknown as FindingOut;
}

describe('FindingsTable layout', () => {
  it('gives the Summary column a minimum width on its header and cells', () => {
    render(
      <MemoryRouter>
        <FindingsTable findings={[row()]} showProvenance />
      </MemoryRouter>,
    );
    const th = screen.getByRole('columnheader', { name: /summary/i });
    expect(th.className).toMatch(/min-w-\[16rem\]/);
    const td = screen.getByText('SQL injection in the order lookup').closest('td');
    expect(td?.className).toMatch(/min-w-\[16rem\]/);
  });

  it('truncates long File:Line, Concern and Target cells, keeping the full value as title', () => {
    render(
      <MemoryRouter>
        <FindingsTable findings={[row()]} showProvenance />
      </MemoryRouter>,
    );
    const loc = screen.getByTitle(`${LONG_PATH}:10–12`);
    expect(loc.className).toMatch(/truncate/);
    expect(loc.className).toMatch(/max-w-\[/);
    for (const text of ['a-very-long-concern-identifier-for-the-ledger', 'target-with-a-long-name-0123456789']) {
      const el = screen.getByTitle(text);
      expect(el.className).toMatch(/truncate/);
      expect(el.className).toMatch(/max-w-\[/);
    }
  });

  it('clips the agent cell', () => {
    render(
      <MemoryRouter>
        <FindingsTable findings={[row()]} showProvenance />
      </MemoryRouter>,
    );
    const tr = screen.getByText('SQL injection in the order lookup').closest('tr')!;
    const agentTd = Array.from(tr.querySelectorAll('td')).find(
      (td) => /heron/i.test(td.textContent ?? '') && td.querySelector('span[class*="max-w-"][class*="truncate"]'),
    );
    expect(agentTd).toBeTruthy();
  });
});
