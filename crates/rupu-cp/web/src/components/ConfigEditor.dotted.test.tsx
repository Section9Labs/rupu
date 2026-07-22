// @vitest-environment jsdom
// Canonical dotted-key encoding — the config key SEGMENT for a pricing model
// id like `/raid/models/zai-org/GLM-5.2-FP8` contains a literal `.`. A plain
// `dotted.split('.')` tears that segment apart, which was the root cause of
// the pricing tab silently rendering a locked-in GLM model row with empty
// $/Mtok fields (the values were there in `eff`, `getPath` just couldn't find
// them under the naively-split key). This file covers the fix: `quoteSegment`
// / `splitDottedKey` round-trip a dotted segment, `getPath` resolves through
// it, and `PricingTab` renders the operator's real shape with values intact.

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, fireEvent } from '@testing-library/react';
import { getPath, quoteSegment, splitDottedKey, PricingTab, ProvidersTab } from './ConfigEditor';

afterEach(() => {
  cleanup();
});

const GLM_MODEL = '/raid/models/zai-org/GLM-5.2-FP8';
const GLM_DOTTED_KEY = `pricing.oracle."${GLM_MODEL}".input_per_mtok`;

describe('quoteSegment', () => {
  it('leaves a plain segment bare', () => {
    expect(quoteSegment('oracle')).toBe('oracle');
    expect(quoteSegment('default_model')).toBe('default_model');
  });

  it('quotes a segment containing a dot', () => {
    expect(quoteSegment(GLM_MODEL)).toBe(`"${GLM_MODEL}"`);
  });

  it('escapes an embedded quote', () => {
    expect(quoteSegment('has"quote')).toBe('"has\\"quote"');
  });
});

describe('splitDottedKey', () => {
  it('splits a dotted key with a quoted, dot-bearing segment', () => {
    expect(splitDottedKey(GLM_DOTTED_KEY)).toEqual([
      'pricing',
      'oracle',
      GLM_MODEL,
      'input_per_mtok',
    ]);
  });

  it('splits simple keys exactly like a naive split', () => {
    expect(splitDottedKey('autoflow.max_active')).toEqual(['autoflow', 'max_active']);
    expect(splitDottedKey('default_model')).toEqual(['default_model']);
  });

  it('falls back to a naive split on an unterminated quote instead of throwing', () => {
    expect(() => splitDottedKey('pricing.oracle."unterminated')).not.toThrow();
    expect(splitDottedKey('pricing.oracle."unterminated')).toEqual(
      'pricing.oracle."unterminated'.split('.'),
    );
  });

  it('falls back to a naive split when a quote does not span the whole segment', () => {
    const malformed = 'pricing.oracle."a"b.input_per_mtok';
    expect(() => splitDottedKey(malformed)).not.toThrow();
    expect(splitDottedKey(malformed)).toEqual(malformed.split('.'));
  });

  // Read-side leniency (deliberate asymmetry with the write-side
  // `split_dotted_key`, which REJECTS every one of these as an empty
  // segment — see that Rust function's doc comment). Locked in here so a
  // future edit can't accidentally tighten the read path and silently
  // change what a malformed/empty key resolves to.
  it('is lenient about empty segments from a leading/trailing/doubled separator', () => {
    expect(splitDottedKey('.')).toEqual(['', '']);
    expect(splitDottedKey('a.')).toEqual(['a', '']);
    expect(splitDottedKey('.x')).toEqual(['', 'x']);
    expect(splitDottedKey('a..b')).toEqual(['a', '', 'b']);
    expect(splitDottedKey('"x".')).toEqual(['x', '']);
  });
});

describe('getPath', () => {
  it('resolves a dotted key with a quoted model segment against the raw (unquoted) object shape', () => {
    const eff = {
      pricing: {
        oracle: {
          [GLM_MODEL]: { input_per_mtok: 1.42 },
        },
      },
    };
    expect(getPath(eff, GLM_DOTTED_KEY)).toBe(1.42);
  });

  it('still resolves simple dotted paths', () => {
    const eff = { autoflow: { max_active: 3 } };
    expect(getPath(eff, 'autoflow.max_active')).toBe(3);
  });
});

describe('PricingTab', () => {
  const noop = () => {};

  it('renders the GLM model heading and its $/Mtok values intact (regression for the empty-fields bug)', () => {
    const eff = {
      pricing: {
        oracle: {
          [GLM_MODEL]: {
            input_per_mtok: 1.42,
            output_per_mtok: 1.42,
            cached_input_per_mtok: 0.82,
          },
        },
        agents: {},
      },
    };

    function fieldValue(key: string): unknown {
      return getPath(eff, key);
    }

    render(
      <PricingTab
        eff={eff}
        prov={{}}
        lockList={[]}
        fieldValue={fieldValue}
        onChange={noop}
        onToggleLock={noop}
      />,
    );

    expect(screen.getByText(GLM_MODEL)).toBeInTheDocument();
    expect(screen.getByText('oracle')).toBeInTheDocument();

    const inputField = screen.getByLabelText('Input $/Mtok') as HTMLInputElement;
    const outputField = screen.getByLabelText('Output $/Mtok') as HTMLInputElement;
    const cachedField = screen.getByLabelText('Cached input $/Mtok') as HTMLInputElement;
    expect(inputField.value).toBe('1.42');
    expect(outputField.value).toBe('1.42');
    expect(cachedField.value).toBe('0.82');
  });
});

describe('ProvidersTab', () => {
  it('quotes a dotted provider alias when building the dotted key (same bug, one tab over)', () => {
    // `Config.providers` is a free-form `BTreeMap<String, ProviderConfig>` —
    // an alias like `azure.eastus` reproduces the exact read-miss /
    // write-corruption the pricing fix addresses if the tab interpolates it
    // unquoted.
    const ALIAS = 'azure.eastus';
    const eff = {
      providers: {
        [ALIAS]: {
          kind: 'azure',
          base_url: 'https://eastus.example',
          default_model: null,
          org_id: null,
          region: null,
          timeout_ms: null,
          max_retries: null,
          max_concurrency: null,
          stream: null,
        },
      },
    };

    function fieldValue(key: string): unknown {
      return getPath(eff, key);
    }
    const onChange = vi.fn();

    render(
      <ProvidersTab
        eff={eff}
        prov={{}}
        lockList={[]}
        fieldValue={fieldValue}
        onChange={onChange}
        onToggleLock={() => {}}
      />,
    );

    // The group heading is the raw (unquoted) alias.
    expect(screen.getByText(ALIAS)).toBeInTheDocument();

    // Only resolves through `getPath` if the tab quoted the alias when
    // building the dotted key it looks itself up under.
    const baseUrl = screen.getByLabelText('Base URL') as HTMLInputElement;
    expect(baseUrl.value).toBe('https://eastus.example');

    fireEvent.change(baseUrl, { target: { value: 'https://new.example' } });
    expect(onChange).toHaveBeenCalledWith(`providers."${ALIAS}".base_url`, 'https://new.example');
  });
});
