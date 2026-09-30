import { useContext } from 'react';
import { ThemeContext } from '../theme/ThemeProvider';

/** Resolved theme mode; 'light' when no provider is mounted (tests). */
export function useThemeMode(): 'light' | 'dark' {
  return useContext(ThemeContext)?.mode ?? 'light';
}
