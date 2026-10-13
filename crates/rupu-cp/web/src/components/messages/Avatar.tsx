import { User } from 'lucide-react';
import { Shape } from '../codename/RoleBadge';
import { useThemeMode } from '../codename/useThemeMode';
import { hexA, identityOf, OPERATOR, roleWordOf } from '../../lib/messages';

/** A participant's avatar: a ring in its identity colour around its role
 *  badge shape (the operator gets a person glyph). */
export function Avatar({ address, size = 24, dim }: { address: string; size?: number; dim?: boolean }) {
  const mode = useThemeMode();
  const { color, shape } = identityOf(address, mode);
  const glyph = Math.round(size * 0.46);
  return (
    <span
      className="inline-flex shrink-0 items-center justify-center rounded-full"
      style={{
        width: size,
        height: size,
        background: hexA(color, 0.14),
        boxShadow: `inset 0 0 0 1px ${hexA(color, 0.45)}`,
        opacity: dim ? 0.5 : 1,
      }}
      title={address}
      aria-hidden
    >
      {address === OPERATOR ? (
        <User size={glyph + 2} style={{ color }} />
      ) : shape ? (
        <svg viewBox="0 0 12 12" width={glyph} height={glyph}>
          <Shape shape={shape} color={color} />
        </svg>
      ) : (
        <span className="font-semibold" style={{ color, fontSize: Math.max(9, size * 0.4) }}>
          {roleWordOf(address).slice(0, 1).toUpperCase()}
        </span>
      )}
    </span>
  );
}

/** An address in its identity colour (`lynx#1`, `role:recon`, `everyone`). */
export function AddressPill({ address, label }: { address: string; label?: string }) {
  const mode = useThemeMode();
  const text = label ?? (address === 'broadcast' ? 'everyone' : address);
  if (address === 'broadcast') {
    return <span className="rounded bg-surface px-1 font-medium text-ink-dim">@{text}</span>;
  }
  const { color } = identityOf(address, mode);
  return (
    <span className="rounded px-1 font-medium" style={{ background: hexA(color, 0.14), color }}>
      @{text}
    </span>
  );
}

export function useIdentityColor(address: string): string {
  return identityOf(address, useThemeMode()).color;
}
