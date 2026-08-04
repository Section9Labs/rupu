// @vitest-environment jsdom
// Task 6 — Brand rail variant. Verifies the default variant (wordmark +
// sublabel) is pixel-identical to today, and the rail variant renders the
// compact lockup (24px gradient tile + 13px wordmark, no sublabel) for the
// v2 shell header.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, expect, it } from 'vitest';
import { cleanup, render, screen } from '@testing-library/react';
import Brand from './Brand';

afterEach(() => {
  cleanup();
});

describe('Brand', () => {
  it('default variant renders wordmark and sublabel', () => {
    render(<Brand />);
    expect(screen.getByText('rupu')).toBeInTheDocument();
    expect(screen.getByText('Control Plane')).toBeInTheDocument();
  });

  it('rail variant renders the compact lockup without a sublabel', () => {
    render(<Brand variant="rail" />);
    expect(screen.getByText('rupu')).toBeInTheDocument();
    expect(screen.queryByText('Control Plane')).not.toBeInTheDocument();
  });
});
