import type { ReactNode } from 'react';
import { StatusIcon } from './StatusIcon';

interface EmptyStateProps {
  title: string;
  description: string;
  action?: ReactNode;
  icon?: ReactNode;
}

export function EmptyState({ title, description, action, icon }: EmptyStateProps) {
  return (
    <section className="imagedb-empty-state">
      <span className="imagedb-empty-state__icon">
        {icon ?? <StatusIcon name="empty" size={28} />}
      </span>
      <h2>{title}</h2>
      <p>{description}</p>
      {action && <div className="imagedb-empty-state__action">{action}</div>}
    </section>
  );
}
