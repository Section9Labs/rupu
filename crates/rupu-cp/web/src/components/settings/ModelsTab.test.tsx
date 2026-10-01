// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, fireEvent, waitFor } from '@testing-library/react';
import { api, ApiError, type CatalogProvider } from '../../lib/api';
import { ModelsTab } from './ModelsTab';

const CATALOG: CatalogProvider[] = [
  {
    provider: 'anthropic',
    fetched_at: new Date(Date.now() - 12 * 60_000).toISOString(),
    stale: false,
    models: [{ id: 'claude-demo-1', input_tokens: 1_000_000, output_tokens: 128_000, source: 'live' }],
  },
  { provider: 'gemini', fetched_at: null, stale: false, models: [] },
];

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe('ModelsTab', () => {
  it('renders each provider with its limits and freshness', async () => {
    vi.spyOn(api, 'getModelCatalog').mockResolvedValue(CATALOG);
    render(<ModelsTab />);
    expect(await screen.findByText('claude-demo-1')).toBeInTheDocument();
    expect(screen.getByText('1,000,000')).toBeInTheDocument();
    expect(screen.getByText('128,000')).toBeInTheDocument();
    expect(screen.getByText(/fetched 12m ago/)).toBeInTheDocument();
    expect(screen.getByText(/never fetched/)).toBeInTheDocument();
  });

  it('Refetch on a provider refreshes just that provider and reloads', async () => {
    const list = vi.spyOn(api, 'getModelCatalog').mockResolvedValue(CATALOG);
    const refresh = vi.spyOn(api, 'refreshModels').mockResolvedValue([{ provider: 'anthropic', ok: true, count: 1 }]);
    render(<ModelsTab />);
    fireEvent.click(await screen.findByRole('button', { name: 'Refetch anthropic' }));
    await waitFor(() => expect(refresh).toHaveBeenCalledWith('anthropic'));
    await waitFor(() => expect(list).toHaveBeenCalledTimes(2));
  });

  it('Refetch all refreshes every provider', async () => {
    vi.spyOn(api, 'getModelCatalog').mockResolvedValue(CATALOG);
    const refresh = vi.spyOn(api, 'refreshModels').mockResolvedValue([]);
    render(<ModelsTab />);
    fireEvent.click(await screen.findByRole('button', { name: 'Refetch all' }));
    await waitFor(() => expect(refresh).toHaveBeenCalledWith(undefined));
  });

  it('shows a provider refresh error inline', async () => {
    vi.spyOn(api, 'getModelCatalog').mockResolvedValue(CATALOG);
    vi.spyOn(api, 'refreshModels').mockResolvedValue([
      { provider: 'gemini', ok: false, count: 0, error: 'no live model-list endpoint' },
    ]);
    render(<ModelsTab />);
    fireEvent.click(await screen.findByRole('button', { name: 'Refetch gemini' }));
    expect(await screen.findByText(/gemini: no live model-list endpoint/)).toBeInTheDocument();
  });

  it('explains when the CP is not cp serve', async () => {
    vi.spyOn(api, 'getModelCatalog').mockRejectedValue(new ApiError(501, 'the model catalog requires `rupu cp serve`'));
    render(<ModelsTab />);
    expect(await screen.findByText(/requires `rupu cp serve`/)).toBeInTheDocument();
  });
});
