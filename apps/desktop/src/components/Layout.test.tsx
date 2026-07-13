import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { cleanup, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, test, vi } from 'vitest';
import type { Route } from '../hooks/use-router';
import { api } from '../lib/ipc/api';
import { Layout } from './Layout';

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

function renderLayout(currentRoute: Route) {
  vi.spyOn(api, 'getDatabaseInfoDashboard').mockResolvedValue({
    database: {
      mode: 'managed_local',
      status: 'running',
      pgvector_available: true,
      migration_version: '0014',
    },
    library: { library_root_count: 1, library_album_count: 0, library_image_count: 0 },
    imports: {
      import_run_count: 0,
      import_album_count: 0,
      import_image_count: 0,
      pending_review_count: 0,
      failed_album_count: 0,
      recovery_required_run_count: 0,
      failed_run_count: 0,
      frozen_plan_count: 0,
    },
    latest_run: null,
    latest_actionable_run: null,
    next_action: 'new_import',
  });
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <Layout currentRoute={currentRoute} onNavigate={vi.fn()} enablePolling={false}>
        <h1>{currentRoute === 'library' ? '图库明细' : '测试页面'}</h1>
      </Layout>
    </QueryClientProvider>,
  );
}

describe('Layout navigation semantics', () => {
  test('marks the dashboard as the current page on the dashboard route', () => {
    renderLayout('dashboard');

    expect(screen.getByRole('button', { name: '工作台' })).toHaveAttribute('aria-current', 'page');
  });

  test('keeps the library visually under dashboard without announcing dashboard as current', () => {
    renderLayout('library');

    const dashboard = screen.getByRole('button', { name: '工作台' });
    expect(dashboard).toHaveClass('is-active');
    expect(dashboard).not.toHaveAttribute('aria-current');
    expect(screen.getByRole('heading', { level: 1, name: '图库明细' })).toBeVisible();
  });

  test('does not change aria-current semantics for other primary routes', () => {
    renderLayout('review');

    expect(screen.getByRole('button', { name: '审核' })).toHaveAttribute('aria-current', 'page');
    expect(screen.getByRole('button', { name: '工作台' })).not.toHaveAttribute('aria-current');
    expect(screen.getByRole('button', { name: '工作台' })).not.toHaveClass('is-active');
  });
});
