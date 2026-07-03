import type { ReactNode } from 'react';
import type { Route } from '../hooks/use-router';

interface LayoutProps {
  children: ReactNode;
  currentRoute: Route;
  onNavigate: (route: Route) => void;
}

const NAV_ITEMS: { route: Route; label: string }[] = [
  { route: 'dashboard', label: '工作台' },
  { route: 'scan', label: '新建导入' },
  { route: 'review', label: '审核' },
  { route: 'commit', label: '入库' },
  { route: 'settings', label: '设置' },
  { route: 'probes', label: '技术探针' },
];

export function Layout({ children, currentRoute, onNavigate }: LayoutProps) {
  return (
    <div className="layout">
      <aside className="sidebar">
        <div className="sidebar-brand">
          <h2>ImageDB</h2>
        </div>
        <nav className="sidebar-nav">
          {NAV_ITEMS.map((item) => (
            <button
              key={item.route}
              className={`nav-item ${currentRoute === item.route ? 'active' : ''}`}
              onClick={() => onNavigate(item.route)}
            >
              {item.label}
            </button>
          ))}
        </nav>
      </aside>
      <main className="main-content">{children}</main>
    </div>
  );
}
