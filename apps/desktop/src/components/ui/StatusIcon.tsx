export type StatusIconName = 'check' | 'info' | 'warning' | 'error' | 'empty';

interface StatusIconProps {
  name: StatusIconName;
  size?: number;
}

export function StatusIcon({ name, size = 18 }: StatusIconProps) {
  const commonProps = {
    width: size,
    height: size,
    viewBox: '0 0 24 24',
    fill: 'none',
    stroke: 'currentColor',
    strokeWidth: 2,
    strokeLinecap: 'round' as const,
    strokeLinejoin: 'round' as const,
    'aria-hidden': true,
  };

  if (name === 'check') {
    return (
      <svg {...commonProps}>
        <path d="M20 6 9 17l-5-5" />
      </svg>
    );
  }

  if (name === 'warning') {
    return (
      <svg {...commonProps}>
        <path d="M10.3 3.7 2.4 17.2A2 2 0 0 0 4.1 20h15.8a2 2 0 0 0 1.7-2.8L13.7 3.7a2 2 0 0 0-3.4 0Z" />
        <path d="M12 9v4" />
        <path d="M12 17h.01" />
      </svg>
    );
  }

  if (name === 'error') {
    return (
      <svg {...commonProps}>
        <circle cx="12" cy="12" r="9" />
        <path d="m9 9 6 6" />
        <path d="m15 9-6 6" />
      </svg>
    );
  }

  if (name === 'empty') {
    return (
      <svg {...commonProps}>
        <rect x="3" y="5" width="18" height="14" rx="3" />
        <path d="m7 15 3-3 2 2 3-4 2 3" />
        <circle cx="8" cy="9" r="1" />
      </svg>
    );
  }

  return (
    <svg {...commonProps}>
      <circle cx="12" cy="12" r="9" />
      <path d="M12 11v5" />
      <path d="M12 8h.01" />
    </svg>
  );
}
