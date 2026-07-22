// @vitest-environment jsdom
// Shared control primitives for the Agent Builder card UI (Task 5). Pure
// render/interaction tests only — no CSS assertions (styles.css `.ab-*`
// block is verified visually, not by these unit tests).

import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, fireEvent } from '@testing-library/react';
import { ChipsInput, Segmented, Scale, LabeledRow } from './controls';

afterEach(cleanup);

describe('ChipsInput', () => {
  it('renders each list item as a chip', () => {
    render(<ChipsInput list={['read_file', 'grep']} onChange={vi.fn()} />);
    expect(screen.getByText('read_file')).toBeInTheDocument();
    expect(screen.getByText('grep')).toBeInTheDocument();
  });

  it('typing a value and pressing Enter calls onChange with the item appended', () => {
    const onChange = vi.fn();
    render(<ChipsInput list={['read_file']} placeholder="add tool…" onChange={onChange} />);
    const input = screen.getByPlaceholderText('add tool…');
    fireEvent.change(input, { target: { value: 'grep' } });
    fireEvent.keyDown(input, { key: 'Enter' });
    expect(onChange).toHaveBeenCalledWith(['read_file', 'grep']);
  });

  it('does not call onChange on Enter with an empty/whitespace draft', () => {
    const onChange = vi.fn();
    render(<ChipsInput list={['read_file']} onChange={onChange} />);
    const input = screen.getByRole('textbox');
    fireEvent.change(input, { target: { value: '   ' } });
    fireEvent.keyDown(input, { key: 'Enter' });
    expect(onChange).not.toHaveBeenCalled();
  });

  it('clicking a chip\'s × button calls onChange without that item', () => {
    const onChange = vi.fn();
    render(<ChipsInput list={['read_file', 'grep']} onChange={onChange} />);
    fireEvent.click(screen.getByRole('button', { name: /remove read_file/i }));
    expect(onChange).toHaveBeenCalledWith(['grep']);
  });

  it('renders not-yet-present suggestions as clickable ghost chips that add on click', () => {
    const onChange = vi.fn();
    render(<ChipsInput list={['read_file']} suggestions={['read_file', 'grep', 'glob']} onChange={onChange} />);
    // read_file already present -> no ghost suggestion for it
    expect(screen.queryByText('+ read_file')).not.toBeInTheDocument();
    const ghost = screen.getByText('+ grep');
    fireEvent.click(ghost);
    expect(onChange).toHaveBeenCalledWith(['read_file', 'grep']);
  });
});

describe('Segmented', () => {
  const options = [
    { label: 'anthropic', value: 'anthropic' as const },
    { label: 'openai', value: 'openai' as const },
  ];

  it('marks the matching option aria-pressed="true" and others "false"', () => {
    render(<Segmented options={options} value="anthropic" onChange={vi.fn()} />);
    expect(screen.getByRole('button', { name: 'anthropic' })).toHaveAttribute('aria-pressed', 'true');
    expect(screen.getByRole('button', { name: 'openai' })).toHaveAttribute('aria-pressed', 'false');
  });

  it('clicking another option calls onChange with its value', () => {
    const onChange = vi.fn();
    render(<Segmented options={options} value="anthropic" onChange={onChange} />);
    fireEvent.click(screen.getByRole('button', { name: 'openai' }));
    expect(onChange).toHaveBeenCalledWith('openai');
  });
});

describe('Scale', () => {
  const options = ['low', 'medium', 'high'];

  it('marks the matching option aria-pressed="true"', () => {
    render(<Scale options={options} value="medium" onChange={vi.fn()} />);
    expect(screen.getByRole('button', { name: 'medium' })).toHaveAttribute('aria-pressed', 'true');
    expect(screen.getByRole('button', { name: 'low' })).toHaveAttribute('aria-pressed', 'false');
  });

  it('clicking another option calls onChange with its value', () => {
    const onChange = vi.fn();
    render(<Scale options={options} value="medium" onChange={onChange} />);
    fireEvent.click(screen.getByRole('button', { name: 'high' }));
    expect(onChange).toHaveBeenCalledWith('high');
  });
});

describe('LabeledRow', () => {
  it('renders the label, optional mono yamlKey, children, and hint', () => {
    render(
      <LabeledRow label="Permission mode" yamlKey="permissionMode" hint="Read-only: mutating tools blocked.">
        <div>control-here</div>
      </LabeledRow>,
    );
    expect(screen.getByText('Permission mode')).toBeInTheDocument();
    expect(screen.getByText('permissionMode')).toBeInTheDocument();
    expect(screen.getByText('control-here')).toBeInTheDocument();
    expect(screen.getByText('Read-only: mutating tools blocked.')).toBeInTheDocument();
  });

  it('omits the yamlKey span and hint when not provided', () => {
    render(
      <LabeledRow label="Name">
        <div>x</div>
      </LabeledRow>,
    );
    expect(screen.queryByText('permissionMode')).not.toBeInTheDocument();
  });
});
