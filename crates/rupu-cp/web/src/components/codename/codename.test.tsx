// @vitest-environment jsdom
import { describe, expect, it } from 'vitest';
import { render } from '@testing-library/react';
import { AgentName } from './AgentName';
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
  it('CrewChip derived is muted with title', () => {
    const { container } = render(<CrewChip crew="cobalt-harbor" derived />);
    const el = container.querySelector('[title="derived for a run recorded before codenames"]');
    expect(el).not.toBeNull();
    expect(el!.className).toContain('opacity-60');
  });
});
