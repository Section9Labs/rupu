// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { afterEach, describe, expect, it } from 'vitest';
import { cleanup, render, screen } from '@testing-library/react';
import { Field } from './Field';

afterEach(cleanup);

describe('Field', () => {
  it('renders label, help, origin chip and the control', () => {
    render(
      <Field label="Default provider" origin="global" help="Used when a run doesn't name one.">
        <input defaultValue="anthropic" aria-label="provider" />
      </Field>,
    );
    expect(screen.getByText('Default provider')).toBeInTheDocument();
    expect(screen.getByText('global')).toBeInTheDocument();
    expect(screen.getByText("Used when a run doesn't name one.")).toBeInTheDocument();
    expect(screen.getByLabelText('provider')).toBeInTheDocument();
  });

  it('shows a locked-by-policy note when locked', () => {
    render(<Field label="Permission mode" locked />);
    expect(screen.getByText('locked by policy')).toBeInTheDocument();
  });

  it('omits the lock note when not locked', () => {
    render(<Field label="Permission mode" />);
    expect(screen.queryByText('locked by policy')).toBeNull();
  });
});
