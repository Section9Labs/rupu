// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, fireEvent, waitFor, within } from '@testing-library/react';
import { api, ApiError, type CatalogProvider, type RefreshOutcome } from '../../lib/api';
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

/** The `<section>` that holds a provider's heading, button, error and table. */
const sectionFor = (provider: string) =>
  screen.getByRole('heading', { name: provider }).closest('section') as HTMLElement;

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

  it('offers Retry when the initial load fails and recovers on success', async () => {
    const list = vi
      .spyOn(api, 'getModelCatalog')
      .mockRejectedValueOnce(new ApiError(500, 'boom', '{"error":"catalog exploded"}'))
      .mockResolvedValueOnce(CATALOG);
    render(<ModelsTab />);
    expect(await screen.findByText('catalog exploded')).toBeInTheDocument();
    expect(list).toHaveBeenCalledTimes(1);

    fireEvent.click(screen.getByRole('button', { name: 'Retry' }));
    expect(await screen.findByText('claude-demo-1')).toBeInTheDocument();
    expect(list).toHaveBeenCalledTimes(2);
    expect(screen.queryByText('catalog exploded')).not.toBeInTheDocument();
  });

  it('renders the stale badge only for a stale provider', async () => {
    vi.spyOn(api, 'getModelCatalog').mockResolvedValue([
      { ...CATALOG[0], stale: true },
      CATALOG[1],
    ]);
    render(<ModelsTab />);
    await screen.findByText('claude-demo-1');
    expect(screen.getAllByText('stale')).toHaveLength(1);
    expect(within(sectionFor('anthropic')).getByText('stale')).toBeInTheDocument();
    expect(within(sectionFor('gemini')).queryByText('stale')).not.toBeInTheDocument();
  });

  it('renders an unknown (null) limit as a dash', async () => {
    vi.spyOn(api, 'getModelCatalog').mockResolvedValue([
      {
        provider: 'anthropic',
        fetched_at: new Date().toISOString(),
        stale: false,
        models: [{ id: 'claude-unknown-1', input_tokens: null, output_tokens: null, source: 'baked-in' }],
      },
    ]);
    render(<ModelsTab />);
    const row = (await screen.findByText('claude-unknown-1')).closest('tr') as HTMLElement;
    expect(within(row).getAllByText('—')).toHaveLength(2);
  });

  it('disables every Refetch button while a refetch is in flight', async () => {
    vi.spyOn(api, 'getModelCatalog').mockResolvedValue(CATALOG);
    let resolveRefresh: (o: RefreshOutcome[]) => void = () => {};
    vi.spyOn(api, 'refreshModels').mockReturnValue(
      new Promise<RefreshOutcome[]>((resolve) => {
        resolveRefresh = resolve;
      }),
    );
    render(<ModelsTab />);
    fireEvent.click(await screen.findByRole('button', { name: 'Refetch anthropic' }));
    await waitFor(() => expect(screen.getByRole('button', { name: 'Refetch anthropic' })).toBeDisabled());
    expect(screen.getByRole('button', { name: 'Refetch gemini' })).toBeDisabled();
    expect(screen.getByRole('button', { name: 'Refetch all' })).toBeDisabled();
    expect(within(screen.getByRole('button', { name: 'Refetch anthropic' })).getByText('Refetching…')).toBeInTheDocument();

    resolveRefresh([{ provider: 'anthropic', ok: true, count: 1 }]);
    await waitFor(() => expect(screen.getByRole('button', { name: 'Refetch anthropic' })).toBeEnabled());
    expect(screen.getByRole('button', { name: 'Refetch gemini' })).toBeEnabled();
    expect(screen.getByRole('button', { name: 'Refetch all' })).toBeEnabled();
  });

  it('clears a provider inline error after a later successful refetch of it', async () => {
    vi.spyOn(api, 'getModelCatalog').mockResolvedValue(CATALOG);
    vi.spyOn(api, 'refreshModels')
      .mockResolvedValueOnce([{ provider: 'gemini', ok: false, count: 0, error: 'no live model-list endpoint' }])
      .mockResolvedValueOnce([{ provider: 'gemini', ok: true, count: 3 }]);
    render(<ModelsTab />);
    fireEvent.click(await screen.findByRole('button', { name: 'Refetch gemini' }));
    expect(await screen.findByText(/gemini: no live model-list endpoint/)).toBeInTheDocument();
    await waitFor(() => expect(screen.getByRole('button', { name: 'Refetch gemini' })).toBeEnabled());

    fireEvent.click(screen.getByRole('button', { name: 'Refetch gemini' }));
    await waitFor(() => expect(screen.queryByText(/no live model-list endpoint/)).not.toBeInTheDocument());
  });

  it("does not show one provider's inline error under another provider", async () => {
    vi.spyOn(api, 'getModelCatalog').mockResolvedValue(CATALOG);
    vi.spyOn(api, 'refreshModels').mockResolvedValue([
      { provider: 'gemini', ok: false, count: 0, error: 'no live model-list endpoint' },
    ]);
    render(<ModelsTab />);
    fireEvent.click(await screen.findByRole('button', { name: 'Refetch gemini' }));
    expect(await within(sectionFor('gemini')).findByText(/gemini: no live model-list endpoint/)).toBeInTheDocument();
    expect(within(sectionFor('anthropic')).queryByRole('alert')).not.toBeInTheDocument();
    expect(within(sectionFor('anthropic')).queryByText(/no live model-list endpoint/)).not.toBeInTheDocument();
  });

  it('keeps the table and shows a banner when a reload fails after a successful load', async () => {
    const list = vi
      .spyOn(api, 'getModelCatalog')
      .mockResolvedValueOnce(CATALOG)
      .mockRejectedValueOnce(new ApiError(500, 'boom', '{"error":"catalog exploded"}'));
    vi.spyOn(api, 'refreshModels').mockResolvedValue([{ provider: 'anthropic', ok: true, count: 1 }]);
    render(<ModelsTab />);
    fireEvent.click(await screen.findByRole('button', { name: 'Refetch anthropic' }));
    expect(await screen.findByText('catalog exploded')).toBeInTheDocument();
    expect(list).toHaveBeenCalledTimes(2);
    // The previously loaded table is still on screen.
    expect(screen.getByText('claude-demo-1')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Refetch all' })).toBeInTheDocument();
  });

  it('shows a thrown single-provider refetch inline under that provider, not as a page banner', async () => {
    vi.spyOn(api, 'getModelCatalog').mockResolvedValue(CATALOG);
    vi.spyOn(api, 'refreshModels').mockRejectedValue(
      new ApiError(400, 'bad request', '{"error":"unknown provider \'nope\': expected one of anthropic, gemini"}'),
    );
    render(<ModelsTab />);
    fireEvent.click(await screen.findByRole('button', { name: 'Refetch anthropic' }));
    const inline = await within(sectionFor('anthropic')).findByText(/unknown provider 'nope'/);
    expect(inline).toBeInTheDocument();
    // Exactly one alert on the page, and it lives inside the provider's section.
    const alerts = screen.getAllByRole('alert');
    expect(alerts).toHaveLength(1);
    expect(sectionFor('anthropic')).toContainElement(alerts[0]);
    expect(within(sectionFor('gemini')).queryByRole('alert')).not.toBeInTheDocument();
  });

  it('shows a thrown Refetch all as a page-level banner', async () => {
    vi.spyOn(api, 'getModelCatalog').mockResolvedValue(CATALOG);
    vi.spyOn(api, 'refreshModels').mockRejectedValue(new ApiError(500, 'boom', '{"error":"refresh exploded"}'));
    render(<ModelsTab />);
    fireEvent.click(await screen.findByRole('button', { name: 'Refetch all' }));
    const banner = await screen.findByText('refresh exploded');
    expect(banner).toBeInTheDocument();
    expect(sectionFor('anthropic')).not.toContainElement(banner);
    expect(sectionFor('gemini')).not.toContainElement(banner);
  });
});
