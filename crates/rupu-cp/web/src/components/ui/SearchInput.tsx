// SearchInput — "find by text" control. Icon-left magnifier, mirroring
// `code/FileNavigator.tsx`'s search field (the best existing search style;
// promoted here so the ~3 bespoke search fields converge on one look).
// Forwards native `<input>` props; `value`/`onChange` are the only ones a
// caller strictly needs to supply.

import { Search } from 'lucide-react';
import { type InputHTMLAttributes } from 'react';
import { cn } from '../../lib/cn';

export type SearchInputProps = InputHTMLAttributes<HTMLInputElement>;

export function SearchInput({ className, placeholder, ...rest }: SearchInputProps) {
  return (
    <div className="relative">
      <Search
        size={12}
        aria-hidden="true"
        className="pointer-events-none absolute left-2 top-1/2 -translate-y-1/2 text-ink-mute"
      />
      <input
        type="text"
        placeholder={placeholder ?? 'Search…'}
        className={cn(
          'w-full rounded border border-border bg-surface py-1 pl-6 pr-2 text-ui text-ink',
          'placeholder:text-ink-mute focus:outline-none focus:ring-1 focus:ring-border',
          className,
        )}
        {...rest}
      />
    </div>
  );
}

export default SearchInput;
