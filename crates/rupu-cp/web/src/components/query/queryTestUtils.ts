// Test helpers for list pages on the query line.
import { fireEvent, screen } from '@testing-library/react';

/** Type `text` into the query line labelled `label` and commit it. */
export function commitQuery(label: string, text: string): void {
  const input = screen.getByRole('combobox', { name: label });
  fireEvent.change(input, { target: { value: text } });
  fireEvent.keyDown(input, { key: 'Enter' });
}

/** The text of each body row's first cell, in order. */
export function firstCells(): string[] {
  return screen
    .getAllByRole('row')
    .filter((r) => r.querySelector('td'))
    .map((r) => r.querySelector('td')?.textContent ?? '');
}
