import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import type { DatabaseInfoDashboard, DatabaseState } from '../../lib/ipc/types';
import { DashboardPage } from '../../pages/DashboardPage';
import { Layout } from '../Layout';

const databaseStatus: DatabaseState = {
  mode: 'managed_local',
  status: 'connected',
  managed_config: {
    data_dir: 'D:/ImageDB/PostgreSQL/data',
    port: 5432,
    username: 'imagedb',
    database: 'imagedb',
  },
  external_config: null,
  pgvector_available: true,
  migration_version: '0012_album_workflow_repair',
  diagnostics: [],
};

const databaseInfo: DatabaseInfoDashboard = {
  database: {
    mode: 'managed_local',
    status: 'connected',
    pgvector_available: true,
    migration_version: '0012_album_workflow_repair',
  },
  library: { library_root_count: 1, library_album_count: 42, library_image_count: 8808 },
  imports: {
    import_run_count: 7,
    import_album_count: 56,
    import_image_count: 12450,
    pending_review_count: 3,
    failed_album_count: 1,
    recovery_required_run_count: 0,
    failed_run_count: 1,
    frozen_plan_count: 4,
  },
  latest_run: {
    import_run_id: 'fixture-run-dashboard',
    source_root: 'D:/照片归档/2026 夏日旅行（待整理）/来自相机与手机的原始照片',
    state: 'analyzing',
    total_albums: 6,
    pending_albums: 1,
    analyzing_albums: 1,
    analyzed_albums: 3,
    review_required_albums: 1,
    failed_albums: 0,
    total_images: 808,
    pending_reviews: 3,
    duplicate_candidates: 28,
  },
  latest_actionable_run: {
    import_run_id: 'fixture-run-dashboard',
    source_root: 'D:/照片归档/2026 夏日旅行（待整理）/来自相机与手机的原始照片',
    state: 'analyzing',
    total_albums: 6,
    pending_albums: 1,
    analyzing_albums: 1,
    analyzed_albums: 3,
    review_required_albums: 1,
    failed_albums: 0,
    total_images: 808,
    pending_reviews: 3,
    duplicate_candidates: 28,
    next_action: 'resume_analysis',
    has_frozen_plan: false,
    has_recoverable_transaction: false,
    has_terminal_unresolved_transaction: false,
    has_missing_plan_album_transaction: false,
  },
  next_action: 'resume_analysis',
};

const fixtureClient = new QueryClient({
  defaultOptions: { queries: { staleTime: Infinity, retry: false } },
});

fixtureClient.setQueryData(['database-status'], databaseStatus);
fixtureClient.setQueryData(['database-info-dashboard'], databaseInfo);

export function DashboardFixture() {
  return (
    <QueryClientProvider client={fixtureClient}>
      <Layout currentRoute="dashboard" onNavigate={() => undefined} enablePolling={false}>
        <DashboardPage
          needsOnboarding={false}
          onConfigureDatabase={() => undefined}
          onGoScan={() => undefined}
          onGoReview={() => undefined}
          onGoCommit={() => undefined}
          onGoRecovery={() => undefined}
          enablePolling={false}
        />
      </Layout>
    </QueryClientProvider>
  );
}
