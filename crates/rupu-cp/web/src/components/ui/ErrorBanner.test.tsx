// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { afterEach, describe, expect, it } from 'vitest';
import { cleanup, render, screen } from '@testing-library/react';
import { ErrorBanner } from './ErrorBanner';

afterEach(cleanup);

describe('ErrorBanner', () => {
  it('renders children inside an alert role', () => {
    render(<ErrorBanner>Something broke</ErrorBanner>);
    const alert = screen.getByRole('alert');
    expect(alert).toHaveTextContent('Something broke');
  });

  it('applies the err/err-bg tone classes', () => {
    render(<ErrorBanner>oops</ErrorBanner>);
    const alert = screen.getByRole('alert');
    expect(alert.className).toMatch(/border-err\/30/);
    expect(alert.className).toMatch(/bg-err-bg/);
    expect(alert.className).toMatch(/text-err/);
  });

  it('merges an extra className', () => {
    render(<ErrorBanner className="mt-4">oops</ErrorBanner>);
    expect(screen.getByRole('alert').className).toMatch(/mt-4/);
  });
});
