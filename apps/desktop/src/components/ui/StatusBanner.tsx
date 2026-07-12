import type { ReactNode } from 'react';
import { StatusIcon, type StatusIconName } from './StatusIcon';
import type { StatusTone } from './StatusBadge';

interface StatusBannerProps {
  tone?: StatusTone;
  title: string;
  children?: ReactNode;
  actions?: ReactNode;
}

const toneIcon: Record<StatusTone, StatusIconName> = {
  neutral: 'info',
  info: 'info',
  success: 'check',
  warning: 'warning',
  danger: 'error',
};

export function StatusBanner({ tone = 'info', title, children, actions }: StatusBannerProps) {
  return (
    <section
      className={`imagedb-status-banner imagedb-status-banner--${tone}`}
      role={tone === 'danger' ? 'alert' : 'status'}
    >
      <span className="imagedb-status-banner__icon">
        <StatusIcon name={toneIcon[tone]} size={20} />
      </span>
      <div className="imagedb-status-banner__content">
        <strong>{title}</strong>
        {children && <div className="imagedb-status-banner__description">{children}</div>}
      </div>
      {actions && <div className="imagedb-status-banner__actions">{actions}</div>}
    </section>
  );
}
