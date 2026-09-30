import { roleBadge } from '../../lib/codename';
import type { BadgeShape } from '../../lib/codenamePalette.gen';
import { useThemeMode } from './useThemeMode';

function Shape({ shape, color }: { shape: BadgeShape; color: string }) {
  const fill = { fill: color };
  switch (shape) {
    case 'circle':
      return <circle cx="6" cy="6" r="5" {...fill} />;
    case 'triangle':
      return <polygon points="6,1 11,11 1,11" {...fill} />;
    case 'inv-triangle':
      return <polygon points="1,1 11,1 6,11" {...fill} />;
    case 'square':
      return <rect x="1.5" y="1.5" width="9" height="9" {...fill} />;
    case 'diamond':
      return <polygon points="6,0.5 11.5,6 6,11.5 0.5,6" {...fill} />;
    case 'pentagon':
      return <polygon points="6,0.8 11.2,4.6 9.2,10.8 2.8,10.8 0.8,4.6" {...fill} />;
    case 'hexagon':
      return <polygon points="3.2,1.2 8.8,1.2 11.6,6 8.8,10.8 3.2,10.8 0.4,6" {...fill} />;
    case 'star':
      return (
        <polygon
          points="6,0.6 7.5,4.3 11.4,4.6 8.4,7.1 9.4,11 6,8.9 2.6,11 3.6,7.1 0.6,4.6 4.5,4.3"
          {...fill}
        />
      );
    case 'cross':
      return <polygon points="4,1 8,1 8,4 11,4 11,8 8,8 8,11 4,11 4,8 1,8 1,4 4,4" {...fill} />;
    case 'ring':
      return <circle cx="6" cy="6" r="4" fill="none" stroke={color} strokeWidth="2" />;
    case 'chevron':
      return <polygon points="1,2 6,7 11,2 11,5.5 6,10.5 1,5.5" {...fill} />;
    case 'half':
      return (
        <>
          <circle cx="6" cy="6" r="4.6" fill="none" stroke={color} strokeWidth="1.2" />
          <path d="M6 1.4 A4.6 4.6 0 0 1 6 10.6 Z" {...fill} />
        </>
      );
  }
}

export function RoleBadge({ role, size = 12 }: { role: string; size?: number }) {
  const mode = useThemeMode();
  const b = roleBadge(role, mode);
  if (!b) return null;
  return (
    <svg
      viewBox="0 0 12 12"
      width={size}
      height={size}
      aria-hidden
      className="inline-block shrink-0"
      data-shape={b.shape}
    >
      <Shape shape={b.shape} color={b.color} />
    </svg>
  );
}
