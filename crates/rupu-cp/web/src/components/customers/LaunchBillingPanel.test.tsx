// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, cleanup, render, screen } from '@testing-library/react';
import { api, ApiError, type PreviewResponse } from '../../lib/api';
import { LaunchBillingPanel } from './LaunchBillingPanel';

const ACME = {
  slug: 'acme-corp',
  name: 'Acme Corp',
  tint: { light: '#2563eb', dark: '#60a5fa' },
  archived: false,
};

const RESP: PreviewResponse = {
  customer: ACME,
  accounts: [
    { role: 'provider', account: 'acme-anthropic', kind: 'anthropic', auth_mode: 'api-key', agents: ['a'], source: 'customer default' },
    { role: 'fallback', account: 'acme-openai', kind: 'openai', agents: ['a'], source: 'customer [recovery].fallbacks' },
    { role: 'scm', account: 'acme-gh', kind: 'github', agents: [], source: 'rule owner = acme-corp' },
  ],
  warnings: ['origin is not a github.com remote'],
};

beforeEach(() => {
  vi.useFakeTimers();
});
afterEach(() => {
  cleanup();
  vi.useRealTimers();
  vi.restoreAllMocks();
});

async function flush(ms = 300) {
  await act(async () => {
    await vi.advanceTimersByTimeAsync(ms);
  });
}

describe('LaunchBillingPanel', () => {
  it('requests the preview with the launcher body after the debounce and renders the rows', async () => {
    const spy = vi.spyOn(api, 'launchPreview').mockResolvedValue(RESP);
    const onResult = vi.fn();
    render(<LaunchBillingPanel body={{ workflow: 'audit', working_dir: '/w' }} onResult={onResult} />);
    expect(spy).not.toHaveBeenCalled();
    await flush();
    expect(spy).toHaveBeenCalledTimes(1);
    expect(spy.mock.calls[0][0]).toEqual({ workflow: 'audit', working_dir: '/w' });

    expect(screen.getByText(/This run uses/)).toHaveTextContent("Acme Corp's accounts");
    expect(screen.getByText('Provider')).toBeInTheDocument();
    expect(screen.getByText('Fallbacks')).toBeInTheDocument();
    expect(screen.getByText('SCM')).toBeInTheDocument();
    expect(screen.getByText('acme-anthropic')).toBeInTheDocument();
    expect(screen.getByText('api-key')).toBeInTheDocument();
    expect(screen.getByText('customer default')).toBeInTheDocument();
    expect(screen.getByText('rule owner = acme-corp')).toBeInTheDocument();
    expect(screen.getByText('origin is not a github.com remote')).toBeInTheDocument();
    expect(onResult).toHaveBeenLastCalledWith({ customer: ACME, blocked: false });
  });

  it('says "the global accounts" when the customer is null', async () => {
    vi.spyOn(api, 'launchPreview').mockResolvedValue({ ...RESP, customer: null });
    render(<LaunchBillingPanel body={{ agent: 'a' }} />);
    await flush();
    expect(screen.getByText(/This run uses/)).toHaveTextContent('the global accounts');
  });

  it('shows the echoed host with its warning', async () => {
    vi.spyOn(api, 'launchPreview').mockResolvedValue({
      ...RESP,
      host: 'mini',
      warnings: ['preview resolves against the machine running the control plane'],
    });
    render(<LaunchBillingPanel body={{ agent: 'a', host: 'mini' }} />);
    await flush();
    expect(screen.getByText(/mini/)).toBeInTheDocument();
    expect(screen.getByText(/machine running the control plane/)).toBeInTheDocument();
  });

  it('a 409 shows the server message and reports blocked', async () => {
    vi.spyOn(api, 'launchPreview').mockRejectedValue(new ApiError(409, 'customer "gone" no longer exists'));
    const onResult = vi.fn();
    render(<LaunchBillingPanel body={{ agent: 'a' }} onResult={onResult} />);
    await flush();
    expect(screen.getByRole('alert')).toHaveTextContent('customer "gone" no longer exists');
    expect(onResult).toHaveBeenLastCalledWith({ customer: undefined, blocked: true });
  });

  it('a 400 shows the message without blocking', async () => {
    vi.spyOn(api, 'launchPreview').mockRejectedValue(new ApiError(400, 'bad name'));
    const onResult = vi.fn();
    render(<LaunchBillingPanel body={{ agent: 'a' }} onResult={onResult} />);
    await flush();
    expect(screen.getByRole('alert')).toHaveTextContent('bad name');
    expect(onResult).toHaveBeenLastCalledWith({ customer: undefined, blocked: false });
  });

  it('debounces rapid changes into one request and aborts the in-flight one', async () => {
    const signals: AbortSignal[] = [];
    const spy = vi.spyOn(api, 'launchPreview').mockImplementation(
      (_b, signal) => {
        if (signal) signals.push(signal);
        return new Promise(() => {});
      },
    );
    const { rerender } = render(<LaunchBillingPanel body={{ agent: 'a', working_dir: '/1' }} />);
    await flush(100);
    rerender(<LaunchBillingPanel body={{ agent: 'a', working_dir: '/2' }} />);
    await flush(100);
    expect(spy).not.toHaveBeenCalled();
    await flush(200);
    expect(spy).toHaveBeenCalledTimes(1);
    expect(spy.mock.calls[0][0]).toEqual({ agent: 'a', working_dir: '/2' });

    rerender(<LaunchBillingPanel body={{ agent: 'a', working_dir: '/3' }} />);
    expect(signals[0].aborted).toBe(true);
    await flush();
    expect(spy).toHaveBeenCalledTimes(2);
    expect(spy.mock.calls[1][0]).toEqual({ agent: 'a', working_dir: '/3' });
  });

  it('a stale response never overwrites a newer one', async () => {
    let resolveFirst!: (r: PreviewResponse) => void;
    const spy = vi
      .spyOn(api, 'launchPreview')
      .mockImplementationOnce(() => new Promise((res) => { resolveFirst = res; }))
      .mockResolvedValueOnce({ ...RESP, customer: null });
    const { rerender } = render(<LaunchBillingPanel body={{ agent: 'a', working_dir: '/1' }} />);
    await flush();
    rerender(<LaunchBillingPanel body={{ agent: 'a', working_dir: '/2' }} />);
    await flush();
    expect(spy).toHaveBeenCalledTimes(2);
    expect(screen.getByText(/This run uses/)).toHaveTextContent('the global accounts');
    await act(async () => {
      resolveFirst(RESP);
    });
    expect(screen.getByText(/This run uses/)).toHaveTextContent('the global accounts');
  });

  it('renders nothing and requests nothing without a body', async () => {
    const spy = vi.spyOn(api, 'launchPreview').mockResolvedValue(RESP);
    const { container } = render(<LaunchBillingPanel body={null} />);
    await flush();
    expect(spy).not.toHaveBeenCalled();
    expect(container).toBeEmptyDOMElement();
  });
});
