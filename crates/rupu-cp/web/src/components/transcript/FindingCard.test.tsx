// @vitest-environment jsdom
import { afterEach, it, expect } from 'vitest';
import { render, screen, cleanup } from '@testing-library/react';
import FindingCard from './FindingCard';
import type { FindingView } from './transcriptView';

afterEach(cleanup);

const HIGH_FINDING: FindingView = {
  severity: 'high',
  summary: 'Hardcoded AES-256 key compiled into the binary',
  scope: 'file',
  filePath: 'crypto-svc/keyring.rs',
  lineRange: [42, 42],
  concernId: 'stride:tampering',
  rationale: 'A static 32-byte AES key is embedded in the binary at line 42.',
  codeExcerpt: 'const KEY: [u8;32] = [0x4a, 0x1f];',
  references: ['https://cwe.mitre.org/data/definitions/798.html'],
};

it('renders the severity label HIGH for a high finding', () => {
  render(<FindingCard finding={HIGH_FINDING} />);
  expect(screen.getByText('HIGH')).not.toBeNull();
});

it('renders the summary text', () => {
  render(<FindingCard finding={HIGH_FINDING} />);
  expect(screen.getByText('Hardcoded AES-256 key compiled into the binary')).not.toBeNull();
});

it('renders the file location chip with line range', () => {
  render(<FindingCard finding={HIGH_FINDING} />);
  expect(screen.getByText('crypto-svc/keyring.rs:42-42')).not.toBeNull();
});

it('renders scope and concern_id chips', () => {
  render(<FindingCard finding={HIGH_FINDING} />);
  expect(screen.getByText('file')).not.toBeNull();
  expect(screen.getByText('stride:tampering')).not.toBeNull();
});

it('renders code excerpt in a pre block', () => {
  render(<FindingCard finding={HIGH_FINDING} />);
  const pre = screen.getByText('const KEY: [u8;32] = [0x4a, 0x1f];');
  expect(pre.tagName.toLowerCase()).toBe('pre');
});

it('renders references as links', () => {
  render(<FindingCard finding={HIGH_FINDING} />);
  const link = screen.getByRole('link', {
    name: 'https://cwe.mitre.org/data/definitions/798.html',
  });
  expect(link).not.toBeNull();
  expect((link as HTMLAnchorElement).href).toBe(
    'https://cwe.mitre.org/data/definitions/798.html',
  );
  expect((link as HTMLAnchorElement).target).toBe('_blank');
});

it('omits location chip when filePath absent', () => {
  const finding: FindingView = {
    severity: 'info',
    summary: 'General observation',
    scope: 'repo',
    rationale: 'Nothing critical here.',
    references: [],
  };
  render(<FindingCard finding={finding} />);
  // No mono location span should appear
  expect(screen.queryByText(/:/)).toBeNull();
  expect(screen.getByText('INFO')).not.toBeNull();
});

it('renders a critical finding with CRITICAL label', () => {
  const finding: FindingView = {
    severity: 'critical',
    summary: 'SQL injection via unsanitised input',
    scope: 'file',
    rationale: 'Direct string interpolation into query.',
    references: [],
  };
  render(<FindingCard finding={finding} />);
  expect(screen.getByText('CRITICAL')).not.toBeNull();
});
