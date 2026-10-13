// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { afterEach, describe, it, expect, vi } from 'vitest';
import { render, screen, cleanup, fireEvent, within } from '@testing-library/react';
import Skills from './Skills';
import type { AgentDraft } from '../../../lib/agentBuilder/agentSpec';

afterEach(cleanup);

function draft(over: Partial<AgentDraft> = {}): AgentDraft {
  return { name: 'a', body: 'b', ...over };
}

describe('Skills card', () => {
  it('suggests installed skills plus the wildcards', () => {
    render(<Skills draft={draft()} patch={vi.fn()} skillNames={['pdf', 'house-style']} />);
    const skills = screen.getByTestId('ab-skills');
    expect(within(skills).getByText('+ pdf')).toBeInTheDocument();
    expect(within(skills).getByText('+ house-style')).toBeInTheDocument();
    expect(within(skills).getByText('+ *')).toBeInTheDocument();
    expect(within(skills).getByText('+ project:*')).toBeInTheDocument();
    expect(screen.getByText(/project skills need their exact name or project:\*/)).toBeInTheDocument();
  });

  it('adding a suggestion patches skills', () => {
    const patch = vi.fn();
    render(<Skills draft={draft({ skills: ['pdf'] })} patch={patch} skillNames={['pdf', 'house-style']} />);
    fireEvent.click(within(screen.getByTestId('ab-skills')).getByText('+ house-style'));
    expect(patch).toHaveBeenCalledWith({ skills: ['pdf', 'house-style'] });
  });

  it('adding a typed name patches preloadSkills', () => {
    const patch = vi.fn();
    render(<Skills draft={draft()} patch={patch} skillNames={[]} />);
    const input = within(screen.getByTestId('ab-preload-skills')).getByRole('textbox');
    fireEvent.change(input, { target: { value: 'house-style' } });
    fireEvent.keyDown(input, { key: 'Enter' });
    expect(patch).toHaveBeenCalledWith({ preloadSkills: ['house-style'] });
  });

  it('labels each input for assistive tech', () => {
    const patch = vi.fn();
    render(<Skills draft={draft()} patch={patch} skillNames={[]} />);
    const load = screen.getByLabelText('Skills it may load');
    fireEvent.change(load, { target: { value: 'pdf' } });
    fireEvent.keyDown(load, { key: 'Enter' });
    expect(patch).toHaveBeenCalledWith({ skills: ['pdf'] });
    expect(screen.getByLabelText('Skills always in its prompt')).toBeInTheDocument();
  });

  it('removing a chip patches the list without it', () => {
    const patch = vi.fn();
    render(<Skills draft={draft({ preloadSkills: ['house-style', 'pdf'] })} patch={patch} />);
    fireEvent.click(screen.getByLabelText('remove house-style'));
    expect(patch).toHaveBeenCalledWith({ preloadSkills: ['pdf'] });
  });
});
