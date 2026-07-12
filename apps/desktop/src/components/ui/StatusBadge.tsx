import type { ReactNode } from 'react';
import { StatusIcon, type StatusIconName } from './StatusIcon';

export type StatusTone = 'neutral' | 'info' | 'success' | 'warning' | 'danger';

interface StatusBadgeProps {
  tone?: StatusTone;
  children: ReactNode;
}

const toneIcon: Record<StatusTone, StatusIconName> = {
  neutral: 'info',
  info: 'info',
  success: 'check',
  warning: 'warning',
  danger: 'error',
};

export function StatusBadge({ tone = 'neutral', children }: StatusBadgeProps) {
  return (
    <span className={`imagedb-status-badge imagedb-status-badge--${tone}`}>
      <StatusIcon name={toneIcon[tone]} size={13} />
      <span>{children}</span>
    </span>
  );
}
