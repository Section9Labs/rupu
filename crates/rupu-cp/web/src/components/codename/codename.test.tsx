// @vitest-environment jsdom
import { describe, expect, it } from 'vitest';
import { render } from '@testing-library/react';
import { AgentName } from './AgentName';
import { ThemeContext } from '../theme/ThemeProvider';
import { CrewChip } from './CrewChip';

describe('codename components', () => {
  it('AgentName renders leaf, label, title and badge', () => {
    const { container } = render(
      <AgentName codename="jade-reef/heron#4" agent="security-reviewer" provider="anthropic" model="claude-opus-5-5" />,
    );
    expect(container.textContent).toContain('heron#4 · security-reviewer · anthropic/claude-opus-5-5');
    expect(container.querySelector('[title="jade-reef/heron#4"]')).not.toBeNull();
    expect(container.querySelector('svg')).not.toBeNull();
  });
  it('AgentName showCrew with a crew-only codename renders the crew once', () => {
    const { container, getAllByText } = render(<AgentName codename="jade-reef" showCrew derived />);
    expect(getAllByText('jade-reef')).toHaveLength(1);
    expect(container.textContent).toBe('jade-reef');
    expect(container.querySelector('.opacity-60')).not.toBeNull();
  });
  it('AgentName showCrew with a crew-only codename keeps the agent · provider/model label', () => {
    const { container } = render(<AgentName codename="jade-reef" agent="oracle" provider="anthropic" model="m" showCrew />);
    expect(container.textContent).toBe('jade-reeforacle · anthropic/m');
  });
  it('AgentName showCrew with a member segment shows crew chip + leaf label', () => {
    const { container } = render(<AgentName codename="jade-reef/heron#4" showCrew />);
    expect(container.textContent).toBe('jade-reefheron#4');
  });
  it('CrewChip derived is muted with title', () => {
    const { container } = render(<CrewChip crew="cobalt-harbor" derived />);
    const el = container.querySelector('[title="derived for a run recorded before codenames"]');
    expect(el).not.toBeNull();
    expect(el!.className).toContain('opacity-60');
  });
  it('uses the dark tint under a dark ThemeContext', () => {
    const { container } = render(
      <ThemeContext.Provider value={{ theme: 'dark', mode: 'dark', setTheme: () => {} }}>
        <CrewChip crew="cobalt-harbor" />
      </ThemeContext.Provider>,
    );
    const dot = container.querySelector('span[aria-hidden]') as HTMLElement;
    expect(dot.style.backgroundColor).toBe('rgb(147, 180, 253)');
  });
  it('renders light without a provider', () => {
    const { container } = render(<CrewChip crew="cobalt-harbor" />);
    const dot = container.querySelector('span[aria-hidden]') as HTMLElement;
    expect(dot.style.backgroundColor).toBe('rgb(29, 78, 216)');
  });
});
