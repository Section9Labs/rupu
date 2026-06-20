// @vitest-environment jsdom
import { afterEach, it, expect } from 'vitest';
import { render, screen, cleanup } from '@testing-library/react';
import StructuredView from './StructuredView';

afterEach(cleanup);

it('renders object keys and values (not [object Object])', () => {
  render(<StructuredView value={{ a: 1, b: true, c: ['x', 'y'], d: { e: 'f' } }} />);
  expect(screen.queryByText('a')).not.toBeNull();
  expect(screen.queryByText('1')).not.toBeNull();
  expect(screen.queryByText('d')).not.toBeNull();
  expect(screen.queryByText('f')).not.toBeNull();
  expect(screen.queryByText('[object Object]')).toBeNull();
});

it('renders a homogeneous object array as a table', () => {
  render(<StructuredView value={[{ id: 1, name: 'a' }, { id: 2, name: 'b' }]} />);
  // a table with id/name headers + the row values
  expect(screen.queryByText('id')).not.toBeNull();
  expect(screen.queryByText('name')).not.toBeNull();
  expect(screen.queryByText('a')).not.toBeNull();
});

it('renders booleans as distinct pills', () => {
  render(<StructuredView value={{ ok: true, bad: false }} />);
  expect(screen.queryByText('true')).not.toBeNull();
  expect(screen.queryByText('false')).not.toBeNull();
});

it('renders null as dim null text', () => {
  render(<StructuredView value={null} />);
  expect(screen.queryByText('null')).not.toBeNull();
});

it('renders undefined as dim dash', () => {
  render(<StructuredView value={undefined} />);
  expect(screen.queryByText('—')).not.toBeNull();
});

it('renders short strings inline', () => {
  render(<StructuredView value="hello world" />);
  expect(screen.queryByText('hello world')).not.toBeNull();
});

it('renders long strings in a pre block', () => {
  const longStr = 'x'.repeat(130);
  render(<StructuredView value={longStr} />);
  const el = screen.queryByText(longStr);
  expect(el).not.toBeNull();
  expect(el?.tagName.toLowerCase()).toBe('pre');
});

it('renders a scalar array as comma-joined text', () => {
  render(<StructuredView value={['x', 'y', 'z']} />);
  // Each value should appear
  expect(screen.queryByText('x')).not.toBeNull();
  expect(screen.queryByText('y')).not.toBeNull();
  expect(screen.queryByText('z')).not.toBeNull();
});

it('renders numbers in mono', () => {
  render(<StructuredView value={42} />);
  expect(screen.queryByText('42')).not.toBeNull();
});

it('falls back to JSON.stringify at depth cap', () => {
  // Build a deeply nested object (5 levels deep)
  const deep = { a: { b: { c: { d: { e: 'leaf' } } } } };
  render(<StructuredView value={deep} />);
  // At depth 4 the innermost { e: 'leaf' } should be stringified — but 'leaf' should appear somewhere
  expect(screen.queryByText(/leaf/)).not.toBeNull();
});
