// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { describe, it, expect } from 'vitest';
import { render, screen } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import MetricRow from './MetricRow';

describe('MetricRow', () => {
  it('renders the header and non-null metrics, omits null', () => {
    render(
      <MemoryRouter>
        <MetricRow
          to="/runs/x"
          header={<span>oracle-assessor</span>}
          metrics={[
            { label: 'in', value: '3,180' },
            { label: 'cost', value: '$0.03' },
            { label: 'turns', value: null },
          ]}
        />
      </MemoryRouter>,
    );
    expect(screen.getByText('oracle-assessor')).toBeInTheDocument();
    expect(screen.getByText('3,180')).toBeInTheDocument();
    expect(screen.getByText('cost')).toBeInTheDocument();
    expect(screen.queryByText('turns')).not.toBeInTheDocument();
  });
});
