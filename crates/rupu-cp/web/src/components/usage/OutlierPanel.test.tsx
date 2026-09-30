// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { describe, it, expect, vi, afterEach } from 'vitest';
import { render, screen, cleanup, fireEvent } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { OutlierPanel } from './OutlierPanel';
import type { OutlierRun } from '../../lib/api';

afterEach(() => {
  cleanup();
});

function outlier(overrides: Partial<OutlierRun> = {}): OutlierRun {
  return {
    run_id: 'run-42',
    workflow_name: 'nightly-review',
    cost_usd: 12,
    baseline_usd: 3,
    ratio: 4,
    started_at: new Date().toISOString(),
    ...overrides,
  };
}

function renderPanel(props: Partial<React.ComponentProps<typeof OutlierPanel>> = {}) {
  return render(
    <MemoryRouter>
      <OutlierPanel outliers={[outlier()]} {...props} />
    </MemoryRouter>,
  );
}

describe('OutlierPanel', () => {
  it('renders the empty state with no outliers', () => {
    render(
      <MemoryRouter>
        <OutlierPanel outliers={[]} />
      </MemoryRouter>,
    );
    expect(screen.getByText(/No cost outliers in this window/)).toBeInTheDocument();
  });

  it('does not render an exclude toggle when onToggleRun is absent', () => {
    renderPanel();
    expect(screen.queryByRole('checkbox')).not.toBeInTheDocument();
  });

  it('calls onToggleRun with the row run_id when its toggle is clicked', () => {
    const onToggleRun = vi.fn();
    renderPanel({ onToggleRun, excludedRunIds: new Set() });
    fireEvent.click(screen.getByRole('checkbox', { name: 'run-42' }));
    expect(onToggleRun).toHaveBeenCalledWith('run-42');
  });

  it('renders an excluded run unchecked and visibly muted', () => {
    renderPanel({ onToggleRun: () => {}, excludedRunIds: new Set(['run-42']) });
    const checkbox = screen.getByRole('checkbox', { name: 'run-42' }) as HTMLInputElement;
    expect(checkbox.checked).toBe(false);
    expect(screen.getByText('nightly-review')).toHaveClass('line-through');
  });

  it('names a workflow outlier by its workflow and shows no kind tag', () => {
    renderPanel({ outliers: [outlier({ kind: 'workflow' })] });
    expect(screen.getByText('nightly-review')).toBeInTheDocument();
    expect(screen.queryByText('agent')).not.toBeInTheDocument();
    expect(screen.queryByText('session')).not.toBeInTheDocument();
  });

  it('falls back to the agent name and tags a standalone agent run', () => {
    renderPanel({
      outliers: [outlier({ run_id: 'run-a', kind: 'agent', workflow_name: '', agent: 'reviewer' })],
    });
    expect(screen.getByText('reviewer')).toBeInTheDocument();
    expect(screen.getByText('agent')).toBeInTheDocument();
  });

  it('falls back to the agent name and tags a session turn', () => {
    renderPanel({
      outliers: [outlier({ run_id: 'run-s', kind: 'session', workflow_name: '', agent: 'assistant' })],
    });
    expect(screen.getByText('assistant')).toBeInTheDocument();
    expect(screen.getByText('session')).toBeInTheDocument();
  });

  it('tolerates a server that omits kind/agent (older API): workflow name, no tag', () => {
    renderPanel({ outliers: [outlier()] });
    expect(screen.getByText('nightly-review')).toBeInTheDocument();
    expect(screen.queryByText('agent')).not.toBeInTheDocument();
  });

  describe('link targets by kind', () => {
    it('links a workflow outlier to its run page', () => {
      renderPanel({ outliers: [outlier({ kind: 'workflow' })] });
      expect(screen.getByRole('link', { name: 'nightly-review' })).toHaveAttribute('href', '/runs/run-42');
    });

    it('links a workflow outlier from an older server (no kind) to its run page', () => {
      renderPanel({ outliers: [outlier()] });
      expect(screen.getByRole('link', { name: 'nightly-review' })).toHaveAttribute('href', '/runs/run-42');
    });

    it('links a session outlier to its session page, not /runs (which would 404)', () => {
      renderPanel({
        outliers: [
          outlier({
            run_id: 'run-s',
            kind: 'session',
            workflow_name: '',
            agent: 'assistant',
            session_id: 'sess-9',
            transcript_path: '/t/run-s.jsonl',
          }),
        ],
      });
      expect(screen.getByRole('link', { name: 'assistant' })).toHaveAttribute('href', '/sessions/sess-9');
    });

    it('links an agent outlier to the transcript view (the AgentRuns route form)', () => {
      renderPanel({
        outliers: [
          outlier({
            run_id: 'run-a',
            kind: 'agent',
            workflow_name: '',
            agent: 'reviewer',
            transcript_path: '/home/me/.rupu/transcripts/run a.jsonl',
          }),
        ],
      });
      expect(screen.getByRole('link', { name: 'reviewer' })).toHaveAttribute(
        'href',
        `/transcript?path=${encodeURIComponent('/home/me/.rupu/transcripts/run a.jsonl')}&live=0`,
      );
    });

    it('renders a session/agent outlier as plain text when the server sent no target', () => {
      renderPanel({
        outliers: [
          outlier({ run_id: 'run-s', kind: 'session', workflow_name: '', agent: 'assistant' }),
          outlier({ run_id: 'run-a', kind: 'agent', workflow_name: '', agent: 'reviewer' }),
        ],
      });
      expect(screen.getByText('assistant')).toBeInTheDocument();
      expect(screen.getByText('reviewer')).toBeInTheDocument();
      expect(screen.queryByRole('link')).not.toBeInTheDocument();
    });
  });
});
