// @vitest-environment jsdom
// DefaultsCard — the workflow-level `defaults:` authoring card. Only
// `findings_profile` is editable here; every other `defaults` key (e.g.
// `continue_on_error`, `workspace`) is preserved verbatim and listed read-only.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, fireEvent } from '@testing-library/react';
import DefaultsCard from './DefaultsCard';

afterEach(cleanup);

describe('DefaultsCard', () => {
  it('renders a "Default findings profile" select offering Agent decides / Full report / Summary', () => {
    render(<DefaultsCard rest={{}} onRest={() => {}} />);
    const select = screen.getByLabelText('Default findings profile') as HTMLSelectElement;
    expect(Array.from(select.options).map((o) => o.textContent)).toEqual(['Agent decides', 'Full report', 'Summary']);
    expect(select.value).toBe('');
  });

  it('reflects the current defaults.findings_profile', () => {
    render(<DefaultsCard rest={{ defaults: { findings_profile: 'summary' } }} onRest={() => {}} />);
    expect((screen.getByLabelText('Default findings profile') as HTMLSelectElement).value).toBe('summary');
  });

  it('choosing Summary writes defaults.findings_profile through onRest, preserving siblings', () => {
    const spy = vi.fn();
    render(
      <DefaultsCard
        rest={{ trigger: { on: 'cron', cron: '0 * * * *' }, defaults: { continue_on_error: true } }}
        onRest={spy}
      />,
    );
    fireEvent.change(screen.getByLabelText('Default findings profile'), { target: { value: 'summary' } });
    expect(spy).toHaveBeenCalledWith({
      trigger: { on: 'cron', cron: '0 * * * *' },
      defaults: { continue_on_error: true, findings_profile: 'summary' },
    });
  });

  it('choosing "Agent decides" removes the key, and defaults entirely when nothing else is set', () => {
    const spy = vi.fn();
    render(<DefaultsCard rest={{ defaults: { findings_profile: 'full' } }} onRest={spy} />);
    fireEvent.change(screen.getByLabelText('Default findings profile'), { target: { value: '' } });
    expect(spy).toHaveBeenCalledWith({});
  });

  it('lists other defaults keys read-only', () => {
    render(
      <DefaultsCard
        rest={{ defaults: { continue_on_error: true, findings_profile: 'full', workspace: 'sync' } }}
        onRest={() => {}}
      />,
    );
    const note = screen.getByTestId('defaults-other-keys');
    expect(note).toHaveTextContent('Other defaults (edit in YAML)');
    expect(note).toHaveTextContent('continue_on_error');
    expect(note).toHaveTextContent('workspace');
    expect(note).not.toHaveTextContent('findings_profile');
  });

  it('shows no "other defaults" line when there are none', () => {
    render(<DefaultsCard rest={{ defaults: { findings_profile: 'full' } }} onRest={() => {}} />);
    expect(screen.queryByTestId('defaults-other-keys')).toBeNull();
  });
});
