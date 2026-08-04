// Compact theme switcher for the sidebar footer. Cycles the preference
// System → Light → Dark → System on click. The icon reflects the *preference*
// (Monitor for system, Sun for light, Moon for dark); the title/aria-label name
// the current preference and the resolved mode so the control is fully
// accessible. Styling uses the themed tokens so it reads correctly in both modes.

import { Moon, Monitor, Sun } from 'lucide-react';
import { Button } from '../ui/Button';
import { useTheme, type Theme } from './ThemeProvider';

const ORDER: Theme[] = ['system', 'light', 'dark'];

const ICON = {
  system: Monitor,
  light: Sun,
  dark: Moon,
} as const;

const LABEL: Record<Theme, string> = {
  system: 'System',
  light: 'Light',
  dark: 'Dark',
};

export function ThemeToggle({ variant = 'row' }: { variant?: 'row' | 'icon' }) {
  const { theme, mode, setTheme } = useTheme();

  const next = ORDER[(ORDER.indexOf(theme) + 1) % ORDER.length];
  const Icon = ICON[theme];
  const description =
    theme === 'system' ? `System (${LABEL[mode]})` : LABEL[theme];

  if (variant === 'icon') {
    return (
      <Button
        variant="ghost"
        size="sm"
        onClick={() => setTheme(next)}
        aria-label={`Theme: ${description}. Switch to ${LABEL[next]}.`}
        title={`Theme: ${description}`}
        className="h-[26px] w-[26px] p-0 justify-center text-ink-dim hover:text-ink"
      >
        <Icon size={16} strokeWidth={2} />
      </Button>
    );
  }

  return (
    <Button
      variant="ghost"
      size="sm"
      onClick={() => setTheme(next)}
      aria-label={`Theme: ${description}. Switch to ${LABEL[next]}.`}
      title={`Theme: ${description}`}
      className="w-full justify-start gap-2 text-ink-dim hover:text-ink"
    >
      <Icon size={16} strokeWidth={2} />
      <span>{LABEL[theme]}</span>
    </Button>
  );
}

export default ThemeToggle;
