import { useQuery } from '@tanstack/react-query';
import type { ReactNode } from 'react';
import type { Route } from '../hooks/use-router';
import { api } from '../lib/ipc/api';
import { AppIcon, type AppIconName } from './ui';

interface LayoutProps {
  children: ReactNode;
  currentRoute: Route;
  onNavigate: (route: Route) => void;
  enablePolling?: boolean;
  navigationDisabled?: boolean;
}

const NAV_ITEMS: { route: Route; label: string; icon: AppIconName }[] = [
  { route: 'dashboard', label: '工作台', icon: 'dashboard' },
  { route: 'scan', label: '新建导入', icon: 'import' },
  { route: 'review', label: '审核', icon: 'review' },
  { route: 'commit', label: '入库', icon: 'commit' },
  { route: 'recovery', label: '恢复', icon: 'recovery' },
];

export function Layout({
  children,
  currentRoute,
  onNavigate,
  enablePolling = true,
  navigationDisabled = false,
}: LayoutProps) {
  const isSettingsPage = currentRoute === 'settings';
  const isSettingsSection = isSettingsPage || currentRoute === 'probes';
  const databaseInfo = useQuery({
    queryKey: ['database-info-dashboard'],
    queryFn: api.getDatabaseInfoDashboard,
    refetchInterval: enablePolling ? 3000 : false,
  });

  const counts: Partial<Record<Route, number>> = {
    scan: databaseInfo.data?.imports.failed_album_count ?? 0,
    review: databaseInfo.data?.imports.pending_review_count ?? 0,
    recovery: databaseInfo.data?.imports.recovery_required_run_count ?? 0,
  };

  return (
    <div className="app-shell">
      <aside className="app-sidebar" aria-label="主导航">
        <button
          className="app-brand"
          type="button"
          aria-label="ImageDB 工作台"
          disabled={navigationDisabled}
          onClick={() => onNavigate('dashboard')}
        >
          <span className="app-brand__mark">
            <AppIcon name="brand" size={24} />
          </span>
          <span className="app-brand__name">ImageDB</span>
        </button>

        <nav className="app-nav">
          {NAV_ITEMS.map((item) => {
            const isCurrentPage = currentRoute === item.route;
            const isVisuallyActive =
              isCurrentPage ||
              (currentRoute === 'library' && item.route === 'dashboard') ||
              (currentRoute === 'plan' && item.route === 'review');
            const count = counts[item.route] ?? 0;
            return (
              <button
                key={item.route}
                type="button"
                className={`app-nav__item ${isVisuallyActive ? 'is-active' : ''}`}
                aria-current={isCurrentPage ? 'page' : undefined}
                aria-label={item.label}
                title={item.label}
                disabled={navigationDisabled}
                onClick={() => onNavigate(item.route)}
              >
                <AppIcon name={item.icon} />
                <span className="app-nav__label">{item.label}</span>
                {count > 0 && (
                  <span className="app-nav__badge" aria-hidden="true">
                    {count > 99 ? '99+' : count}
                  </span>
                )}
              </button>
            );
          })}
        </nav>

        <div className="app-sidebar__footer">
          <button
            type="button"
            className={`app-nav__item ${isSettingsSection ? 'is-active' : ''}`}
            aria-current={isSettingsPage ? 'page' : undefined}
            aria-label="设置"
            title="设置"
            disabled={navigationDisabled}
            onClick={() => onNavigate('settings')}
          >
            <AppIcon name="settings" />
            <span className="app-nav__label">设置</span>
          </button>
          <p className="app-sidebar__privacy">
            <span aria-hidden="true">●</span>
            <span>本地处理</span>
          </p>
        </div>
      </aside>
      <main className={`app-main app-main--${currentRoute}`}>{children}</main>
    </div>
  );
}
