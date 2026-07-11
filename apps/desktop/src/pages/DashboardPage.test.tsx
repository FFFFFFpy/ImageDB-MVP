import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, test, vi } from 'vitest';
import type { DatabaseInfoDashboard, DatabaseState } from '../lib/ipc/types';
import { DashboardPage } from './DashboardPage';

const mockState = vi.hoisted(() => ({
  databaseStatus: {
    mode: 'managed_local',
    status: 'connected',
    managed_config: {
      data_dir: 'D:/ImageDB/postgres',
      port: 5432,
      username: 'imagedb',
      database: 'imagedb',
    },
    external_config: null,
    pgvector_available: true,
    migration_version: '0011_album_workflow_state',
    diagnostics: [],
  } as DatabaseState,
  databaseInfo: {
    database: {
      mode: 'managed_local',
      status: 'connected',
      pgvector_available: true,
      migration_version: '0011_album_workflow_state',
    },
    library: {
      library_root_count: 2,
      library_album_count: 8,
      library_image_count: 120,
    },
    imports: {
      import_run_count: 5,
      import_album_count: 14,
      import_image_count: 260,
      pending_review_count: 3,
      failed_album_count: 1,
      recovery_required_run_count: 0,
      failed_run_count: 1,
      frozen_plan_count: 2,
    },
    latest_run: {
      import_run_id: 'run-1',
      source_root: 'D:/Photos',
      state: 'analyzing',
      total_albums: 4,
      pending_albums: 1,
      analyzing_albums: 0,
      analyzed_albums: 2,
      review_required_albums: 1,
      failed_albums: 1,
      total_images: 20,
      pending_reviews: 3,
      duplicate_candidates: 4,
    },
    latest_actionable_run: {
      import_run_id: 'run-1',
      source_root: 'D:/Photos',
      state: 'analyzing',
      total_albums: 4,
      pending_albums: 1,
      analyzing_albums: 0,
      analyzed_albums: 2,
      review_required_albums: 1,
      failed_albums: 1,
      total_images: 20,
      pending_reviews: 3,
      duplicate_candidates: 4,
      next_action: 'resume_analysis',
      has_frozen_plan: false,
      has_active_transaction: false,
    },
    next_action: 'resume_analysis',
  } as DatabaseInfoDashboard,
}));

const mockApi = vi.hoisted(() => ({
  getDatabaseStatus: vi.fn(() => Promise.resolve(mockState.databaseStatus)),
  getDatabaseInfoDashboard: vi.fn(() => Promise.resolve(mockState.databaseInfo)),
}));

vi.mock('../lib/ipc/api', () => ({
  api: mockApi,
}));

function renderDashboard(
  onGoScan = vi.fn(),
  onGoReview = vi.fn(),
  onGoCommit = vi.fn(),
  onGoRecovery = vi.fn(),
) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return {
    onGoScan,
    onGoReview,
    onGoCommit,
    onGoRecovery,
    ...render(
      <QueryClientProvider client={client}>
        <DashboardPage
          needsOnboarding={false}
          onConfigureDatabase={vi.fn()}
          onGoScan={onGoScan}
          onGoReview={onGoReview}
          onGoCommit={onGoCommit}
          onGoRecovery={onGoRecovery}
        />
      </QueryClientProvider>,
    ),
  };
}

beforeEach(() => {
  vi.clearAllMocks();
  mockState.databaseInfo = {
    ...mockState.databaseInfo,
    imports: {
      ...mockState.databaseInfo.imports,
      pending_review_count: 3,
    },
    latest_run: mockState.databaseInfo.latest_run
      ? {
          ...mockState.databaseInfo.latest_run,
          pending_albums: 1,
          pending_reviews: 3,
        }
      : null,
    next_action: 'resume_analysis',
    latest_actionable_run: mockState.databaseInfo.latest_actionable_run
      ? {
          ...mockState.databaseInfo.latest_actionable_run,
          pending_albums: 1,
          pending_reviews: 3,
          next_action: 'resume_analysis',
          has_frozen_plan: false,
          has_active_transaction: false,
        }
      : null,
  };
});

afterEach(() => cleanup());

describe('DashboardPage database info', () => {
  test('renders the database info dashboard counts', async () => {
    renderDashboard();

    expect(await screen.findByText('数据库概览')).toBeInTheDocument();
    expect(screen.getByText('图库根目录')).toBeInTheDocument();
    expect(screen.getByText('已入库图集')).toBeInTheDocument();
    expect(screen.getByText('已入库图片')).toBeInTheDocument();
    expect(screen.getByText('导入任务')).toBeInTheDocument();
    expect(screen.getByText('待审核')).toBeInTheDocument();
    expect(screen.getByText('失败图集')).toBeInTheDocument();
    expect(screen.getByText('冻结计划')).toBeInTheDocument();
    expect(await screen.findByText('120')).toBeInTheDocument();
    expect(await screen.findByText('260')).toBeInTheDocument();
  });

  test('passes the resumable run id when continuing analysis', async () => {
    mockState.databaseInfo = {
      ...mockState.databaseInfo,
      imports: { ...mockState.databaseInfo.imports, pending_review_count: 0 },
      latest_actionable_run: {
        ...mockState.databaseInfo.latest_actionable_run!,
        pending_reviews: 0,
        next_action: 'resume_analysis',
      },
      next_action: 'resume_analysis',
    };
    const { onGoScan } = renderDashboard();

    fireEvent.click(await screen.findByRole('button', { name: '继续分析' }));
    expect(onGoScan).toHaveBeenCalledWith('run-1');
  });

  test('ignores abandoned historical failures and pending reviews for the next action', async () => {
    mockState.databaseInfo = {
      ...mockState.databaseInfo,
      imports: {
        ...mockState.databaseInfo.imports,
        pending_review_count: 0,
        failed_album_count: 0,
      },
      latest_run: {
        ...mockState.databaseInfo.latest_run!,
        state: 'abandoned',
        pending_reviews: 9,
        failed_albums: 2,
      },
      latest_actionable_run: null,
      next_action: 'new_import',
    };
    const { onGoScan, onGoReview } = renderDashboard();

    expect(await screen.findByText(/已放弃/)).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: '查看失败图集' })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: '继续审核' })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: '开始导入' }));
    expect(onGoScan).toHaveBeenCalledWith(null);
    expect(onGoReview).not.toHaveBeenCalled();
  });

  test('routes a new ready-to-commit run to import review despite abandoned history', async () => {
    mockState.databaseInfo = {
      ...mockState.databaseInfo,
      imports: {
        ...mockState.databaseInfo.imports,
        pending_review_count: 0,
        failed_album_count: 0,
      },
      latest_run: { ...mockState.databaseInfo.latest_run!, state: 'abandoned' },
      latest_actionable_run: {
        ...mockState.databaseInfo.latest_actionable_run!,
        import_run_id: 'run-new',
        state: 'ready_to_commit',
        pending_albums: 0,
        pending_reviews: 0,
        failed_albums: 0,
        next_action: 'generate_plan',
      },
      next_action: 'generate_plan',
    };
    const onGoReview = vi.fn();
    renderDashboard(vi.fn(), onGoReview);

    fireEvent.click(await screen.findByRole('button', { name: '前往入库审核' }));
    expect(onGoReview).toHaveBeenCalledOnce();
  });

  test('routes a fully reviewed review-required run to plan generation', async () => {
    mockState.databaseInfo = {
      ...mockState.databaseInfo,
      imports: { ...mockState.databaseInfo.imports, pending_review_count: 0 },
      latest_actionable_run: {
        ...mockState.databaseInfo.latest_actionable_run!,
        state: 'review_required',
        pending_albums: 0,
        analyzing_albums: 0,
        pending_reviews: 0,
        failed_albums: 0,
        next_action: 'generate_plan',
      },
      next_action: 'generate_plan',
    };
    const onGoReview = vi.fn();
    renderDashboard(vi.fn(), onGoReview);

    fireEvent.click(await screen.findByRole('button', { name: '前往入库审核' }));
    expect(onGoReview).toHaveBeenCalledOnce();
    expect(screen.queryByRole('button', { name: '开始导入' })).not.toBeInTheDocument();
  });

  test('routes a cancelled frozen-plan run without an active transaction to commit', async () => {
    mockState.databaseInfo = {
      ...mockState.databaseInfo,
      latest_actionable_run: {
        ...mockState.databaseInfo.latest_actionable_run!,
        state: 'cancelled',
        pending_albums: 0,
        analyzing_albums: 0,
        pending_reviews: 0,
        failed_albums: 0,
        next_action: 'resume_commit',
        has_frozen_plan: true,
        has_active_transaction: false,
      },
      next_action: 'resume_commit',
    };
    const onGoCommit = vi.fn();
    renderDashboard(vi.fn(), vi.fn(), onGoCommit);

    fireEvent.click(await screen.findByRole('button', { name: '继续入库' }));
    expect(onGoCommit).toHaveBeenCalledOnce();
  });

  test('routes committing or active-transaction work to recovery', async () => {
    mockState.databaseInfo = {
      ...mockState.databaseInfo,
      latest_actionable_run: {
        ...mockState.databaseInfo.latest_actionable_run!,
        state: 'committing',
        pending_albums: 0,
        analyzing_albums: 0,
        pending_reviews: 0,
        failed_albums: 0,
        next_action: 'recover',
        has_frozen_plan: true,
        has_active_transaction: true,
      },
      next_action: 'recover',
    };
    const onGoRecovery = vi.fn();
    renderDashboard(vi.fn(), vi.fn(), vi.fn(), onGoRecovery);

    fireEvent.click(await screen.findByRole('button', { name: '前往恢复' }));
    expect(onGoRecovery).toHaveBeenCalledOnce();
  });
});
