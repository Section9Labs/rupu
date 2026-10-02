// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { afterEach, describe, expect, it } from 'vitest';
import { render, screen, cleanup } from '@testing-library/react';
import WarnMark, { unitWarningSuffix, warningTitle, WARNING_TITLE_LIMIT } from './WarnMark';

afterEach(cleanup);

describe('warningTitle', () => {
  it('lists each warning on its own line, unit-scoped ones prefixed', () => {
    expect(warningTitle([{ message: 'a' }, { index: 3, message: 'b' }])).toBe('a\nunit 3: b');
  });

  it('caps a flood at the limit and says how many more there are', () => {
    const many = Array.from({ length: WARNING_TITLE_LIMIT + 7 }, (_, i) => ({ index: i, message: `m${i}` }));
    const lines = warningTitle(many).split('\n');
    expect(lines).toHaveLength(WARNING_TITLE_LIMIT + 1);
    expect(lines[0]).toBe('unit 0: m0');
    expect(lines[WARNING_TITLE_LIMIT - 1]).toBe(`unit ${WARNING_TITLE_LIMIT - 1}: m${WARNING_TITLE_LIMIT - 1}`);
    expect(lines[WARNING_TITLE_LIMIT]).toBe('+7 more');
  });

  it('does not add a +N more line at exactly the limit', () => {
    const exact = Array.from({ length: WARNING_TITLE_LIMIT }, (_, i) => ({ message: `m${i}` }));
    expect(warningTitle(exact)).not.toMatch(/more/);
  });
});

describe('unitWarningSuffix', () => {
  it('is empty without warnings', () => {
    expect(unitWarningSuffix(undefined)).toBe('');
    expect(unitWarningSuffix([])).toBe('');
  });

  it('appends each message behind a ⚠', () => {
    expect(unitWarningSuffix(['x', 'y'])).toBe(' · ⚠ x · ⚠ y');
  });

  it('caps a flood the same way', () => {
    const many = Array.from({ length: WARNING_TITLE_LIMIT + 2 }, (_, i) => `m${i}`);
    const out = unitWarningSuffix(many);
    expect(out).toContain('⚠ m0');
    expect(out).not.toContain(`m${WARNING_TITLE_LIMIT}`);
    expect(out).toMatch(/ · \+2 more$/);
  });
});

describe('WarnMark', () => {
  it('renders nothing without warnings', () => {
    const { container } = render(<WarnMark warnings={[]} />);
    expect(container).toBeEmptyDOMElement();
  });

  it('puts the capped title on the marker', () => {
    const many = Array.from({ length: WARNING_TITLE_LIMIT + 3 }, (_, i) => ({ message: `m${i}` }));
    render(<WarnMark warnings={many} />);
    const mark = screen.getByTestId('rg-warn');
    expect(mark).toHaveAttribute('aria-label', `${WARNING_TITLE_LIMIT + 3} warnings`);
    expect(mark.getAttribute('title')).toMatch(/\n\+3 more$/);
  });
});
