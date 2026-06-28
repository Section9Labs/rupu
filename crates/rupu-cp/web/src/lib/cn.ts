import { clsx, type ClassValue } from 'clsx';
import { extendTailwindMerge } from 'tailwind-merge';

// Register our custom semantic font-size tokens (tailwind.config `fontSize`:
// meta/note/ui/lead) with tailwind-merge. Without this, twMerge doesn't know
// `text-ui` etc. are font-sizes and misclassifies them into the text-COLOR
// group — so a later `text-ui` would strip an earlier `text-white`/`text-red-700`
// (silently killing Button/Badge/StatusPill text colors). Registering them as
// `font-size` makes them conflict only with other sizes, as intended.
const twMerge = extendTailwindMerge({
  extend: {
    classGroups: {
      'font-size': [{ text: ['meta', 'note', 'ui', 'lead'] }],
    },
  },
});

export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs));
}
