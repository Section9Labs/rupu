// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { cleanup, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it } from 'vitest';
import { TagHistory } from './TagHistory';
import type { TagEvent } from '../../../lib/api';

afterEach(cleanup);

const ev = (op: 'add' | 'remove', tag: string, by: TagEvent['by'], at: string): TagEvent => ({ id: `tge_${tag}${op}`, finding_id: 'fnd_a', op, tag, by, at });

describe('TagHistory', () => {
  it('lists newest first with the actor', () => {
    render(
      <TagHistory
        events={[
          ev('add', 'class:sqli', { kind: 'agent', run_id: 'run_1', model: 'm', surface: 'workflow', agent: 'sqli-hunter', codename: 'jade-reef/heron#3' }, '2026-10-06T10:00:00Z'),
          ev('remove', 'false-positive', { kind: 'operator', user: 'alice', via: 'cp' }, '2026-10-06T11:00:00Z'),
        ]}
      />,
    );
    const items = screen.getAllByRole('listitem');
    expect(items[0]).toHaveTextContent('− false-positive');
    expect(items[0]).toHaveTextContent('alice via cp');
    expect(items[1]).toHaveTextContent('+ class:sqli');
    expect(items[1]).toHaveTextContent('jade-reef/heron#3 (sqli-hunter)');
  });
  it('says so when there is no history', () => {
    render(<TagHistory events={[]} />);
    expect(screen.getByText('No tag changes yet.')).toBeInTheDocument();
  });
});
