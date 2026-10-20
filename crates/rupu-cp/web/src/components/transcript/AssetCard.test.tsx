// @vitest-environment jsdom
/**
 * Tests for AssetCard — the `assets.mark` view. Parser cases use the real
 * payloads seen in the cellgw transcripts.
 */

import { it, expect, describe, afterEach } from 'vitest';
import { render, screen, cleanup } from '@testing-library/react';
import { parseAsset, interpretAssetError, AssetSummary, AssetBody } from './AssetCard';
import type { ToolView } from './transcriptView';

afterEach(cleanup);

function tv(overrides: Partial<ToolView>): ToolView {
  return { tool: 'assets.mark', kind: 'asset', input: {}, ...overrides };
}

describe('parseAsset — success', () => {
  const view = parseAsset(
    tv({
      input: {
        coordinates: [
          { t: 'host', v: '84.232.96.70' },
          { t: 'port', v: { number: 443, proto: 'tcp' } },
        ],
        depth: 'tested',
        kind: 'network:service',
        label: 'Opengear Console Manager HTTPS / api v2.2',
      },
      output: 'asset 3f2ab1c9d0e4f5a6 is at depth `tested`',
    }),
  );

  it('extracts kind, depth, label', () => {
    expect(view.kind).toBe('network:service');
    expect(view.depth).toBe('tested');
    expect(view.label).toContain('Opengear');
  });

  it('formats host + port coordinates', () => {
    expect(view.coords).toEqual([
      { tag: 'host', text: '84.232.96.70', valid: true },
      { tag: 'port', text: '443/tcp', valid: true },
    ]);
  });

  it('parses the ledger success line', () => {
    expect(view.assetId).toBe('3f2ab1c9d0e4f5a6');
    expect(view.effectiveDepth).toBe('tested');
    expect(view.error).toBeNull();
  });
});

describe('parseAsset — the real serde rejection (port as a string)', () => {
  const view = parseAsset(
    tv({
      input: {
        coordinates: [
          { t: 'host', v: '103.225.73.177' },
          { t: 'port', v: '123' },
        ],
        depth: 'tested',
        kind: 'network:service',
        label: 'NTP (udp/123)',
      },
      output: '',
      error: 'invalid input: invalid type: string "123", expected struct variant Coordinate::Port',
    }),
  );

  it('flags the malformed port coordinate', () => {
    const port = view.coords.find((c) => c.tag === 'port');
    expect(port).toEqual({ tag: 'port', text: '"123"', valid: false });
  });

  it('explains expected-vs-got in plain language', () => {
    expect(view.error).not.toBeNull();
    expect(view.error!.friendly).toContain('port coordinate must be an object');
    expect(view.error!.friendly).toContain('number, proto');
    expect(view.error!.got).toBe('"123"');
    expect(view.error!.expected).toContain('"number": 443');
    expect(view.error!.raw).toBe('invalid type: string "123", expected struct variant Coordinate::Port');
  });
});

describe('interpretAssetError — other shapes', () => {
  it('integer-typed port', () => {
    const coords = [{ tag: 'port', text: '646', valid: false }];
    const e = interpretAssetError(
      'invalid input: invalid type: integer `646`, expected struct variant Coordinate::Port',
      coords,
    );
    expect(e.friendly).toContain('port coordinate');
    expect(e.got).toBe('646');
  });

  it('missing field maps to the owning tag', () => {
    const coords = [{ tag: 'port', text: '{"number":80}', valid: false }];
    const e = interpretAssetError('missing field `proto`', coords);
    expect(e.friendly).toContain('port coordinate');
  });

  it('unrecognised error falls back to the cleaned message', () => {
    const e = interpretAssetError('invalid input: something odd happened', []);
    expect(e.friendly).toBe('something odd happened');
    expect(e.expected).toBeNull();
    expect(e.got).toBeNull();
  });
});

describe('AssetSummary / AssetBody render', () => {
  it('summary shows kind, coords and depth', () => {
    const view = parseAsset(
      tv({
        input: {
          coordinates: [{ t: 'host', v: '10.0.0.1' }],
          depth: 'tested',
          kind: 'network:service',
        },
        output: 'asset abcd is at depth `tested`',
      }),
    );
    render(<AssetSummary view={view} />);
    expect(screen.getByText('network:service')).toBeTruthy();
    expect(screen.getByText('10.0.0.1')).toBeTruthy();
    expect(screen.getByText('depth tested')).toBeTruthy();
  });

  it('body renders the rejection explanation, not a raw block', () => {
    const view = parseAsset(
      tv({
        input: { coordinates: [{ t: 'port', v: '443' }], kind: 'network:service', label: 'svc' },
        error: 'invalid input: invalid type: string "443", expected struct variant Coordinate::Port',
      }),
    );
    render(<AssetBody view={view} />);
    expect(screen.getByText(/must be an object/)).toBeTruthy();
    expect(screen.getByText('svc')).toBeTruthy();
  });
});
