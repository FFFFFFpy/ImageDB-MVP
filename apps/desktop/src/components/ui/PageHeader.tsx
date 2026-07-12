import type { ReactNode } from 'react';

interface PageHeaderProps {
  title: string;
  description?: string;
  meta?: ReactNode;
  actions?: ReactNode;
}

export function PageHeader({ title, description, meta, actions }: PageHeaderProps) {
  return (
    <header className="imagedb-page-header">
      <div className="imagedb-page-header__copy">
        <div className="imagedb-page-header__title-row">
          <h1>{title}</h1>
          {meta}
        </div>
        {description && <p>{description}</p>}
      </div>
      {actions && <div className="imagedb-page-header__actions">{actions}</div>}
    </header>
  );
}
