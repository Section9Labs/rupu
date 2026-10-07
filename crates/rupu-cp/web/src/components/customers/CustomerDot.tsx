import type { TintDto } from '../../lib/api';
import { useThemeMode } from '../codename/useThemeMode';

/** A customer's color dot. The color comes ONLY from the API's `tint`, picked
 *  by the current theme. `aria-hidden` unless a `title` gives it a name. */
export function CustomerDot({
  tint,
  size = 8,
  title,
}: {
  tint: TintDto;
  size?: number;
  title?: string;
}) {
  const mode = useThemeMode();
  return (
    <span
      data-customer-dot
      aria-hidden={title ? undefined : true}
      title={title}
      className="inline-block shrink-0 rounded-full"
      style={{
        width: size,
        height: size,
        backgroundColor: mode === 'dark' ? tint.dark : tint.light,
      }}
    />
  );
}
