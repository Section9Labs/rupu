// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { afterEach, beforeEach, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, fireEvent } from '@testing-library/react';
import { api } from '../lib/api';
import { rankAndGroup } from './TargetPicker';
import { WORKSPACE_ITEM, type TargetItem } from '../lib/targetItems';
import TargetPicker from './TargetPicker';

// ---------------------------------------------------------------------------
// Pure ranking tests.
// ---------------------------------------------------------------------------

const items: TargetItem[] = [
  WORKSPACE_ITEM,
  { kind: 'project', label: 'rupu', sublabel: '/Code/rupu', resolved: { working_dir: '/Code/rupu' } },
  { kind: 'project', label: 'okesu', sublabel: '/Code/Okesu', resolved: { working_dir: '/Code/Okesu' } },
  { kind: 'repo', label: 'github:acme/api', sublabel: 'main', resolved: { target: 'github:acme/api' } },
];

describe('rankAndGroup', () => {
  it('empty query keeps all, workspace group first', () => {
    const groups = rankAndGroup(items, '');
    expect(groups[0].kind).toBe('workspace');
    expect(groups.map((g) => g.kind)).toEqual(['workspace', 'project', 'repo']);
  });
  it('filters by fuzzy query across label/sublabel', () => {
    const groups = rankAndGroup(items, 'rupu');
    const flat = groups.flatMap((g) => g.items.map((x) => x.item.label));
    expect(flat).toContain('rupu');
    expect(flat).not.toContain('okesu');
  });
});

// ---------------------------------------------------------------------------
// Component blur-commit regression tests.
// ---------------------------------------------------------------------------

describe('TargetPicker blur-commit', () => {
  beforeEach(() => {
    vi.spyOn(api, 'getProjects').mockResolvedValue([]);
    vi.spyOn(api, 'getRepos').mockResolvedValue([]);
    vi.spyOn(api, 'browseDir').mockResolvedValue({ path: '/', parent: null, dirs: [] });
  });

  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
  });

  it('commits a typed directory path on blur without pressing Enter', () => {
    const onChange = vi.fn();
    render(<TargetPicker value={WORKSPACE_ITEM} onChange={onChange} />);

    const input = screen.getByRole('combobox');
    fireEvent.focus(input);
    fireEvent.change(input, { target: { value: '/tmp/projX' } });
    fireEvent.blur(input);

    expect(onChange).toHaveBeenCalledWith(
      expect.objectContaining({ resolved: { working_dir: '/tmp/projX' } }),
    );
  });

  it('commits a typed repo ref on blur without pressing Enter', () => {
    const onChange = vi.fn();
    render(<TargetPicker value={WORKSPACE_ITEM} onChange={onChange} />);

    const input = screen.getByRole('combobox');
    fireEvent.focus(input);
    fireEvent.change(input, { target: { value: 'github:acme/api' } });
    fireEvent.blur(input);

    expect(onChange).toHaveBeenCalledWith(
      expect.objectContaining({ resolved: { target: 'github:acme/api' } }),
    );
  });

  it('reverts to committed value label when query is unresolvable on blur', () => {
    const onChange = vi.fn();
    render(<TargetPicker value={WORKSPACE_ITEM} onChange={onChange} />);

    const input = screen.getByRole('combobox');
    fireEvent.focus(input);
    // Type something that doesn't match any label or freetext pattern.
    fireEvent.change(input, { target: { value: 'notapathatall' } });
    fireEvent.blur(input);

    // onChange should NOT be called since query is unresolvable.
    expect(onChange).not.toHaveBeenCalled();
    // Input should revert to the committed value's label.
    expect((input as HTMLInputElement).value).toBe(WORKSPACE_ITEM.label);
  });

  it('falls back to WORKSPACE_ITEM and resets label when query is cleared', () => {
    const onChange = vi.fn();
    render(<TargetPicker value={WORKSPACE_ITEM} onChange={onChange} />);

    const input = screen.getByRole('combobox');
    fireEvent.focus(input);
    fireEvent.change(input, { target: { value: '' } });
    fireEvent.blur(input);

    expect(onChange).toHaveBeenCalledWith(WORKSPACE_ITEM);
    expect((input as HTMLInputElement).value).toBe(WORKSPACE_ITEM.label);
  });
});
