// A prompt box where `/` at the start of the message opens a menu of the
// installed skills (Claude Code's `/skill-name args`): picking one writes
// `/name ` into the text, which the server turns into an invoked skill.
import { useId, useMemo, useRef, useState, type KeyboardEvent, type TextareaHTMLAttributes } from 'react';
import { cn } from '../../lib/cn';
import { insertSlashSkill, matchSlashSkills, readSlashTokens, slashTokenAt, type SlashSkill } from '../../lib/slashSkills';

type Props = Omit<TextareaHTMLAttributes<HTMLTextAreaElement>, 'value' | 'onChange'> & {
  value: string;
  onChange: (value: string) => void;
  /** The skills the menu offers; empty when they can't be listed (a remote
   *  host), and a typed `/name` still goes through. */
  skills: SlashSkill[];
};

export default function SlashSkillTextarea({ value, onChange, skills, onKeyDown, className, ...rest }: Props) {
  const listId = useId();
  const ref = useRef<HTMLTextAreaElement>(null);
  const [caret, setCaret] = useState(0);
  const [active, setActive] = useState(0);
  const [dismissed, setDismissed] = useState<number | null>(null);

  const token = slashTokenAt(value, Math.min(caret, value.length));
  const options = useMemo(
    () => (token && skills.length > 0 && dismissed !== token.start ? matchSlashSkills(skills, token.query) : []),
    [token?.start, token?.query, skills, dismissed],
  );
  const open = options.length > 0;
  const { invoked, unknown } = useMemo(() => readSlashTokens(value, skills), [value, skills]);

  const track = (el: HTMLTextAreaElement) => setCaret(el.selectionStart ?? el.value.length);

  const pick = (entry: string) => {
    if (!token) return;
    const next = insertSlashSkill(value, token.start, caret, entry);
    onChange(next.text);
    setCaret(next.caret);
    setActive(0);
    requestAnimationFrame(() => ref.current?.setSelectionRange(next.caret, next.caret));
  };

  const keyDown = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    if (open && !e.metaKey && !e.ctrlKey) {
      if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
        e.preventDefault();
        const step = e.key === 'ArrowDown' ? 1 : -1;
        setActive((i) => (i + step + options.length) % options.length);
        return;
      }
      if (e.key === 'Enter' || e.key === 'Tab') {
        e.preventDefault();
        pick(options[Math.min(active, options.length - 1)].entry);
        return;
      }
      if (e.key === 'Escape') {
        // Closes the menu, not a dialog the box sits in.
        e.preventDefault();
        e.stopPropagation();
        setDismissed(token!.start);
        return;
      }
    }
    onKeyDown?.(e);
  };

  return (
    <div className="relative">
      <textarea
        {...rest}
        ref={ref}
        value={value}
        role="combobox"
        aria-autocomplete="list"
        aria-expanded={open}
        aria-controls={listId}
        aria-activedescendant={open ? `${listId}-${active}` : undefined}
        onChange={(e) => {
          onChange(e.target.value);
          track(e.target);
          setActive(0);
          setDismissed(null);
        }}
        onSelect={(e) => track(e.currentTarget)}
        onKeyDown={keyDown}
        onBlur={() => setDismissed(token?.start ?? null)}
        onFocus={() => setDismissed(null)}
        className={className}
      />
      {open && (
        <ul
          id={listId}
          role="listbox"
          aria-label="Skills"
          className="absolute bottom-full left-0 z-30 mb-1 max-h-72 w-full max-w-lg overflow-y-auto rounded-lg border border-border bg-panel py-1 shadow-card"
        >
          {options.map((o, i) => (
            <li
              key={o.entry}
              id={`${listId}-${i}`}
              role="option"
              aria-selected={i === active}
              onMouseDown={(e) => e.preventDefault()}
              onMouseEnter={() => setActive(i)}
              onClick={() => pick(o.entry)}
              className={cn(
                'flex cursor-pointer items-baseline gap-3 px-3 py-1.5',
                i === active ? 'bg-surface-active' : 'hover:bg-surface-hover',
              )}
            >
              <span className="shrink-0 font-mono text-ui text-ink">/{o.entry}</span>
              {o.description && <span className="truncate text-ui text-ink-mute">{o.description}</span>}
            </li>
          ))}
        </ul>
      )}
      {!open && (invoked.length > 0 || unknown) && (
        <p className="mt-1 text-ui text-ink-dim">
          {invoked.length > 0 && (
            <span>
              Invokes{' '}
              {invoked.map((s, i) => (
                <span key={s}>
                  {i > 0 && ', '}
                  <span className="font-mono text-ink">{s}</span>
                </span>
              ))}
              {unknown && '. '}
            </span>
          )}
          {unknown && (
            <span className="text-warn">
              No skill named <span className="font-mono">{unknown}</span>: it is sent as text.
            </span>
          )}
        </p>
      )}
    </div>
  );
}
