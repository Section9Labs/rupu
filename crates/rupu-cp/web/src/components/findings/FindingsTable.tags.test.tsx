// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { afterEach, describe, expect, it } from 'vitest';
import { cleanup, render, screen } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { FindingsTable } from './FindingsTable';
import type { FindingOut } from '../../lib/api';

afterEach(cleanup);

function row(id: string, tags: string[]): FindingOut {
  return {
    id,
    scope: 'repo',
    summary: `summary ${id}`,
    severity: 'high',
    evidence: { rationale: 'r', references: [] },
    declared_by: { run_id: 'r', model: 'm', surface: 'workflow' },
    declared_at: '2026-10-06T00:00:00Z',
    tags,
    ws_id: 'ws1',
    project: 'shop-web',
    target_id: 't',
    codename: 'jade-reef',
    codename_derived: true,
  } as unknown as FindingOut;
}

describe('FindingsTable tags column', () => {
  it('shows up to two tags then +N with the full list as a title', () => {
    render(
      <MemoryRouter>
        <FindingsTable findings={[row('fnd_a', ['class:sqli', 'needs-poc', 'triaged']), row('fnd_b', [])]} showProvenance />
      </MemoryRouter>,
    );
    expect(screen.getByText('class:sqli')).toBeInTheDocument();
    expect(screen.getByText('needs-poc')).toBeInTheDocument();
    expect(screen.queryByText('triaged')).not.toBeInTheDocument();
    expect(screen.getByText('+1')).toHaveAttribute('title', 'class:sqli, needs-poc, triaged');
  });
});
