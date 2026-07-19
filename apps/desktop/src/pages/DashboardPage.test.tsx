import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, test, vi } from 'vitest';
import type { DatabaseInfoDashboard, DatabaseState } from '../lib/ipc/types';
import { DashboardPage, getNextActionPresentation } from './DashboardPage';

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
      has_recoverable_transaction: false,
      has_terminal_unresolved_transaction: false,
      has_missing_plan_album_transaction: false,
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
  onGoLibrary = vi.fn(),
  onGoPlan = vi.fn(),
) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return {
    client,
    onGoScan,
    onGoReview,
    onGoPlan,
    onGoCommit,
    onGoRecovery,
    onGoLibrary,
    ...render(
      <QueryClientProvider client={client}>
        <DashboardPage
          needsOnboarding={false}
          onConfigureDatabase={vi.fn()}
          onGoScan={onGoScan}
          onGoReview={onGoReview}
          onGoPlan={onGoPlan}
          onGoCommit={onGoCommit}
          onGoRecovery={onGoRecovery}
          onGoLibrary={onGoLibrary}
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
          has_recoverable_transaction: false,
          has_terminal_unresolved_transaction: false,
          has_missing_plan_album_transaction: false,
        }
      : null,
  };
});

afterEach(() => cleanup());

describe('DashboardPage database info', () => {
  test('renders the database info dashboard counts', async () => {
    renderDashboard();

    expect(await screen.findByText('图库概览')).toBeInTheDocument();
    expect(screen.getByText('图集')).toBeInTheDocument();
    expect(screen.getByText('图片')).toBeInTheDocument();
    expect(screen.getByText('位置')).toBeInTheDocument();
    expect(screen.getByText('导入任务进度')).toBeInTheDocument();
    expect(screen.getByText('待审核')).toBeInTheDocument();
    expect(screen.getByText('失败')).toBeInTheDocument();
    expect(await screen.findByText('120')).toBeInTheDocument();
  });

  test('preserves keyboard focus when a polling refetch returns updated data', async () => {
    const { client } = renderDashboard();
    const action = await screen.findByRole('button', { name: '继续分析' });
    action.focus();

    mockState.databaseInfo = {
      ...mockState.databaseInfo,
      library: { ...mockState.databaseInfo.library, library_image_count: 121 },
    };
    await client.refetchQueries({ queryKey: ['database-info-dashboard'] });

    await waitFor(() => expect(screen.getByText('121')).toBeInTheDocument());
    expect(action).toHaveFocus();
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
    const { onGoPlan } = renderDashboard();

    fireEvent.click(await screen.findByRole('button', { name: '前往入库调整' }));
    expect(onGoPlan).toHaveBeenCalledOnce();
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
    const { onGoPlan } = renderDashboard();

    fireEvent.click(await screen.findByRole('button', { name: '前往入库调整' }));
    expect(onGoPlan).toHaveBeenCalledOnce();
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
        has_recoverable_transaction: false,
        has_terminal_unresolved_transaction: false,
        has_missing_plan_album_transaction: true,
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
        has_recoverable_transaction: true,
        has_terminal_unresolved_transaction: false,
        has_missing_plan_album_transaction: false,
      },
      next_action: 'recover',
    };
    const onGoRecovery = vi.fn();
    renderDashboard(vi.fn(), vi.fn(), vi.fn(), onGoRecovery);

    fireEvent.click(await screen.findByRole('button', { name: '前往恢复' }));
    expect(onGoRecovery).toHaveBeenCalledOnce();
  });

  test('routes terminal failed or cancelled transactions to explicit manual disposition', async () => {
    mockState.databaseInfo = {
      ...mockState.databaseInfo,
      latest_actionable_run: {
        ...mockState.databaseInfo.latest_actionable_run!,
        state: 'recovery_required',
        pending_albums: 0,
        analyzing_albums: 0,
        pending_reviews: 0,
        failed_albums: 0,
        next_action: 'inspect_transaction_failure',
        has_frozen_plan: true,
        has_recoverable_transaction: false,
        has_terminal_unresolved_transaction: true,
        has_missing_plan_album_transaction: false,
      },
      next_action: 'inspect_transaction_failure',
    };
    const onGoRecovery = vi.fn();
    renderDashboard(vi.fn(), vi.fn(), vi.fn(), onGoRecovery);

    fireEvent.click(await screen.findByRole('button', { name: '处理失败事务' }));
    expect(onGoRecovery).toHaveBeenCalledOnce();
    expect(screen.queryByRole('button', { name: '前往恢复' })).not.toBeInTheDocument();
  });
});

describe('DashboardPage next_action presentation', () => {
  test.each([
    ['recover', '前往恢复'],
    ['inspect_transaction_failure', '处理失败事务'],
    ['review', '继续审核'],
    ['generate_plan', '前往入库调整'],
    ['resume_analysis', '继续分析'],
    ['inspect_failed', '查看失败图集'],
    ['resume_commit', '继续入库'],
    ['new_import', '开始导入'],
  ] as const)('maps backend action %s to %s', (action, label) => {
    expect(getNextActionPresentation(action).label).toBe(label);
  });
});
