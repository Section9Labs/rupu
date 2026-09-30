// @vitest-environment jsdom
// ExportReportButton — the page-level trigger that owns the dialog's open
// state and refuses to open it over an empty set.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { ExportReportButton } from './ExportReportButton';

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe('ExportReportButton', () => {
  it('opens the export dialog over the given rows', () => {
    render(<ExportReportButton findings={[{ id: 'a', profile: 'full' }]} defaultTitle="Findings report" wsId="ws-1" />);
    expect(screen.queryByRole('dialog')).toBeNull();
    fireEvent.click(screen.getByRole('button', { name: 'Export report' }));
    expect(screen.getByRole('dialog')).toBeInTheDocument();
    expect(screen.getByLabelText('Title')).toHaveValue('Findings report');
  });

  it('closes the dialog again', () => {
    render(<ExportReportButton findings={[{ id: 'a', profile: 'full' }]} defaultTitle="T" />);
    fireEvent.click(screen.getByRole('button', { name: 'Export report' }));
    fireEvent.click(screen.getByRole('button', { name: 'Cancel' }));
    expect(screen.queryByRole('dialog')).toBeNull();
  });

  it('returns focus to the trigger when the dialog closes, even if the click never focused it', () => {
    render(<ExportReportButton findings={[{ id: 'a', profile: 'full' }]} defaultTitle="T" />);
    const trigger = screen.getByRole('button', { name: 'Export report' });
    // Safari / Firefox on macOS do not focus a button on click: nothing has focus.
    expect(trigger).not.toHaveFocus();
    fireEvent.click(trigger);
    expect(screen.getByRole('radio', { name: 'Markdown' })).toHaveFocus();

    fireEvent.click(screen.getByRole('button', { name: 'Cancel' }));
    expect(screen.queryByRole('dialog')).toBeNull();
    expect(trigger).toHaveFocus();
  });

  it('also returns focus on Escape', () => {
    render(<ExportReportButton findings={[{ id: 'a', profile: 'full' }]} defaultTitle="T" />);
    const trigger = screen.getByRole('button', { name: 'Export report' });
    fireEvent.click(trigger);
    fireEvent.keyDown(document, { key: 'Escape' });
    expect(trigger).toHaveFocus();
  });

  it('does not steal focus on first render', () => {
    render(<ExportReportButton findings={[{ id: 'a', profile: 'full' }]} defaultTitle="T" />);
    expect(screen.getByRole('button', { name: 'Export report' })).not.toHaveFocus();
  });

  it('is disabled, with a reason, when the filtered set is empty', () => {
    render(<ExportReportButton findings={[]} defaultTitle="T" />);
    const button = screen.getByRole('button', { name: 'Export report' });
    expect(button).toBeDisabled();
    expect(screen.getByTitle('No findings to export')).toBeInTheDocument();
    expect(button).toHaveAccessibleDescription('No findings to export');
    fireEvent.click(button);
    expect(screen.queryByRole('dialog')).toBeNull();
  });
});
